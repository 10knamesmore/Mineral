//! `load()` 主管线:eval default → eval user → 深合并 → 反序列化,永不因用户配置失败。

use std::path::Path;

use mlua::{Function, Lua, LuaSerdeExt, Table, Value};

use crate::{Error, Result};

use crate::loader::lua_util::table_at;
use crate::loader::merge::deep_merge;
use crate::loader::stub::inject_noop_host;
use crate::loader::warning::ConfigWarning;
use crate::schema::{
    COPY_TEMPLATE_FNS, CURATE_PLAYLISTS_MERGED_FN, CURATE_PLAYLISTS_SOURCE_FNS, Config,
    QUEUE_TRANSFORM_FNS,
};

/// 加载用户配置。用户 `config.lua` 的任何错误降级为纯默认 + 一条 [`ConfigWarning`];
/// 仅当内置 `default.lua` 损坏(程序员错误,守卫测试拦截)才返回 `Err`。
///
/// 管线:内置 `default.lua` eval → 用户 `config.lua` eval(缺失则跳过)→ Lua 层深合并
/// → `serde_json` 中转 + `serde_path_to_error` 反序列化。非 daemon 进程已注入 no-op
/// host stub,故用户配置顶层 `mineral.on(...)` 安全。
///
/// # Params:
///   - `user_path`: 用户配置文件路径;不存在视为纯默认。
///
/// # Return:
///   `(Config, warnings)`:warnings 非空 = 用了默认兜底,调用方据此 toast。
pub fn load(user_path: &Path) -> Result<(Config, Vec<ConfigWarning>)> {
    let lua = new_vm()?;
    let (config, warnings, _user_evaled, _tree) = load_on(&lua, user_path)?;
    Ok((config, warnings))
}

/// daemon 侧加载产物(见 [`load_with_vm`])。
pub struct DaemonLoad {
    /// 落型后的强类型配置(用户侧失败时为默认)。
    pub config: Config,

    /// 用户配置的降级告警(非空 = 用了默认兜底,调用方据此提示)。
    pub warnings: Vec<ConfigWarning>,

    /// 用户脚本 eval 且配置落型全部成功时交还的 VM(移交脚本运行时)。
    pub vm: Option<Lua>,

    /// 合成树(default + user 深合并、函数字段已摘;用户侧失败时为默认树)。
    /// 作为推送给 client 的有效配置底树,session 覆盖经
    /// [`merge_tree`](crate::merge_tree) 叠加后再 [`from_tree`](crate::from_tree) 校验。
    pub tree: serde_json::Value,
}

/// daemon 专用:在**带活 host API** 的 VM 上加载 —— `config.lua` 顶层的
/// `mineral.on(...)` 等调用真实注册,成功后把 VM 交还调用方移交脚本运行时。
///
/// 配置与脚本是同一次 eval,**失败同沉**:用户文件缺失 / eval 失败 / 配置
/// 落型失败,一律回落默认配置且不交还 VM(`None`,部分注册过的 VM 弃掉)。
///
/// # Params:
///   - `user_path`: 用户配置文件路径;不存在视为纯默认(也无脚本可跑)。
///   - `install`: 把活 host API 挂进 VM 的回调(daemon 传脚本运行时的安装器;
///     本 crate 不依赖脚本 crate,方向由调用方注入)。
///
/// # Return:
///   [`DaemonLoad`]:`vm` 为 `Some` 仅当用户脚本 eval 且配置落型全部成功。
pub fn load_with_vm(
    user_path: &Path,
    install: impl FnOnce(&Lua) -> mlua::Result<()>,
) -> Result<DaemonLoad> {
    let lua = Lua::new();
    install(&lua).map_err(|source| Error::Lua {
        operation: "安装脚本 API",
        source,
    })?;
    let (config, warnings, user_evaled, tree) = load_on(&lua, user_path)?;
    let vm = user_evaled.then_some(lua);
    Ok(DaemonLoad {
        config,
        warnings,
        vm,
        tree,
    })
}

/// `load` / `load_with_vm` 的共同主体:在给定 VM 上 eval default → eval user
/// → 深合并 → 落型,任何用户侧失败降级为默认 + warning。
///
/// # Params:
///   - `lua`: 已注入 host API(no-op 或活实现)的 VM
///   - `user_path`: 用户配置文件路径
///
/// # Return:
///   `(Config, warnings, user_evaled, tree)`:`user_evaled` 为 true 表示用户文件
///   存在、eval 成功且配置落型成功(VM 内的脚本注册有效);`tree` 是与 `Config`
///   对应的合成树(失败路径为默认树)。
fn load_on(
    lua: &Lua,
    user_path: &Path,
) -> Result<(Config, Vec<ConfigWarning>, bool, serde_json::Value)> {
    let default_table = eval_default(lua)?;
    let mut warnings = Vec::<ConfigWarning>::new();

    let user_table = match eval_user(lua, user_path) {
        Ok(table) => table,
        Err(warning) => {
            warnings.push(warning);
            None
        }
    };

    let Some(user) = user_table else {
        let (config, tree, warnings) = finalize_default(lua, default_table, warnings)?;
        return Ok((config, warnings, false, tree));
    };

    let merged = deep_merge(lua, default_table.clone(), user)?;
    prepare_lua_table(lua, &merged)?;
    match from_lua_table(merged) {
        Ok((config, tree)) => Ok((config, warnings, true, tree)),
        Err(warning) => {
            warnings.push(warning);
            let (config, tree, warnings) = finalize_default(lua, default_table, warnings)?;
            Ok((config, warnings, false, tree))
        }
    }
}

/// 落型前标记数组，并把 Lua 函数字段摘进 VM registry。
/// 所有 `from_lua_table` 调用都经过这里。
///
/// 各提取器共同语义:非 function 的值不摘——留在表里让落型报 unknown field
/// (带路径),比静默吞掉好定位。配置整体落型失败回落默认时 registry 里可能
/// 残留已摘函数,但默认配置不声明这些字段,无键触达,无害。
fn prepare_lua_table(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    if let Some(local) = table_at!(merged, sources.local)
        && let Ok(roots) = local.get::<Table>("roots")
    {
        roots.set_metatable(Some(lua.array_metatable()));
    }
    extract_copy_templates(lua, merged)?;
    extract_playlist_transforms(lua, merged)?;
    extract_queue_transforms(lua, merged)?;
    Ok(())
}

/// 把 `queue.transforms[i].transform` 从配置表里摘出,按数组序存进 VM named registry
/// (键 [`QUEUE_TRANSFORM_FNS`]);表上的 `transform` 字段移除,`key`/`label` 留下进常规
/// 落型。对位方式与 `tui.copy.templates` 相同(见 [`extract_copy_templates`])。
fn extract_queue_transforms(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    let fns = lua.create_table()?;
    if let Some(transforms) = table_at!(merged, queue.transforms) {
        transforms.set_metatable(Some(lua.array_metatable()));
        for i in 1..=transforms.raw_len() {
            let Ok(item) = transforms.get::<Table>(i) else {
                continue;
            };
            if let Ok(f) = item.get::<Function>("transform") {
                fns.raw_set(i, f)?;
                item.raw_set("transform", Value::Nil)?;
            }
        }
    }
    lua.set_named_registry_value(QUEUE_TRANSFORM_FNS, fns)?;
    Ok(())
}

/// 把 `tui.copy.templates[i].template` 从配置表里摘出,按数组序存进 VM named
/// registry(键 [`COPY_TEMPLATE_FNS`]);表上的 `template` 字段移除,
/// `key`/`label`/`context` 留下进常规落型。client 渲染菜单项与 daemon 取函数
/// 执行靠**数组下标对位**(两边 eval 的是同一份 config)。顺手给 `templates`
/// 表挂 array metatable——空 Lua 表经 serde 默认序列化成 map `{}`,落不进
/// `Vec`,挂上才走 `[]`(默认表的空 `templates` 同样需要 metatable 修正)。
fn extract_copy_templates(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    let fns = lua.create_table()?;
    if let Some(templates) = table_at!(merged, tui.copy.templates) {
        templates.set_metatable(Some(lua.array_metatable()));
        for i in 1..=templates.raw_len() {
            let Ok(item) = templates.get::<Table>(i) else {
                continue;
            };
            if let Ok(f) = item.get::<Function>("template") {
                fns.raw_set(i, f)?;
                item.raw_set("template", Value::Nil)?;
            }
        }
    }
    lua.set_named_registry_value(COPY_TEMPLATE_FNS, fns)?;
    Ok(())
}

/// 把两级 `curate_playlists`(Lua function)从 `sources` 表里摘进 VM named
/// registry:per-source(`sources.<name>.curate_playlists`)按 **source 名**
/// 存进 [`CURATE_PLAYLISTS_SOURCE_FNS`] 表;跨源(`sources.curate_playlists`,
/// 合并列表 transform)存 [`CURATE_PLAYLISTS_MERGED_FN`](未声明为 Nil)。
///
/// per-source 条目摘完若变空(该源无配置段,如 `local` 只写了 curate),条目
/// 一并移除——不要求源有 schema 段,`deny_unknown_fields` 不会拒它。拼错的
/// 源名在此无从校验(config crate 不知运行期 channel 集),由 daemon 启动时
/// 对无对应 channel 的键打 warn。
fn extract_playlist_transforms(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    let fns = lua.create_table()?;
    let mut merged_fn = Value::Nil;
    if let Some(sources) = table_at!(merged, sources) {
        if let Ok(f) = sources.get::<Function>("curate_playlists") {
            merged_fn = Value::Function(f);
            sources.raw_set("curate_playlists", Value::Nil)?;
        }
        // 先收集再改:迭代中删 sources 自身的键是未定义行为。
        let mut emptied = Vec::<Value>::new();
        for pair in sources.pairs::<Value, Value>() {
            let (key, value) = pair?;
            let Value::Table(section) = value else {
                continue;
            };
            if let Ok(f) = section.get::<Function>("curate_playlists") {
                fns.raw_set(key.clone(), f)?;
                section.raw_set("curate_playlists", Value::Nil)?;
                if section.is_empty() {
                    emptied.push(key);
                }
            }
        }
        for key in emptied {
            sources.raw_set(key, Value::Nil)?;
        }
    }
    lua.set_named_registry_value(CURATE_PLAYLISTS_SOURCE_FNS, fns)?;
    lua.set_named_registry_value(CURATE_PLAYLISTS_MERGED_FN, merged_fn)?;
    Ok(())
}

impl Config {
    /// 纯默认配置(eval `default.lua`)。仅守卫测试与降级路径用;业务正常路径走 [`load`]。
    ///
    /// # Return:
    ///   内置默认;若 `default.lua` 自身坏(不该发生,有守卫测试)返回 `Err`。
    pub fn defaults() -> Result<Self> {
        let lua = new_vm()?;
        let table = eval_default(&lua)?;
        prepare_lua_table(&lua, &table)?;
        let (config, _tree) =
            from_lua_table(table).map_err(|warning| Error::DefaultConfig { warning })?;
        Ok(config)
    }
}

/// 纯默认配置的合成树(eval `default.lua`,函数字段已摘)。与
/// [`Config::defaults`] 同源;供无真实加载管线的场合(测试)
/// 当配置宿主的静态底树。
///
/// # Return:
///   默认树;`default.lua` 自身坏(不该发生,有守卫测试)返回 `Err`。
pub fn default_tree() -> Result<serde_json::Value> {
    let lua = new_vm()?;
    let table = eval_default(&lua)?;
    prepare_lua_table(&lua, &table)?;
    let (_config, tree) =
        from_lua_table(table).map_err(|warning| Error::DefaultConfig { warning })?;
    Ok(tree)
}

/// 把默认表落成 `Config` 并打包 warnings;default 坏则 fail(程序员错误)。
///
/// # Params:
///   - `lua`: 持有该表的 VM(templates 摘取 / metatable 修正用)
///   - `default_table`: 默认配置表
///   - `warnings`: 已累积的用户配置 warnings
///
/// # Return:
///   `(默认 Config, 默认树, warnings)`
fn finalize_default(
    lua: &Lua,
    default_table: Table,
    warnings: Vec<ConfigWarning>,
) -> Result<(Config, serde_json::Value, Vec<ConfigWarning>)> {
    prepare_lua_table(lua, &default_table)?;
    let (config, tree) =
        from_lua_table(default_table).map_err(|warning| Error::DefaultConfig { warning })?;
    Ok((config, tree, warnings))
}

/// 建 VM 并注入 no-op host stub。
///
/// # Return:
///   就绪的 VM
fn new_vm() -> Result<Lua> {
    let lua = Lua::new();
    inject_noop_host(&lua)?;
    Ok(lua)
}

/// eval 内置 `default.lua`,返回默认表(必成功,守卫测试守)。
///
/// # Params:
///   - `lua`: 目标 VM
///
/// # Return:
///   默认配置表
fn eval_default(lua: &Lua) -> Result<Table> {
    let table: Table = lua
        .load(include_str!("../lua/default.lua"))
        .set_name("default.lua")
        .eval()
        .map_err(|source| Error::Lua {
            operation: "解析 default.lua",
            source,
        })?;
    Ok(table)
}

/// eval 用户文件(若存在)。文件不存在 → `Ok(None)`;读取失败 → `ConfigWarning::Read`,
/// 求值失败 → `ConfigWarning::Eval`。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `path`: 用户配置路径
///
/// # Return:
///   `Ok(Some(table))` 用户表 / `Ok(None)` 文件缺失 / `Err(warning)` 读取或求值失败
fn eval_user(lua: &Lua, path: &Path) -> std::result::Result<Option<Table>, ConfigWarning> {
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigWarning::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let table: Table = lua
        .load(&src)
        .set_name("config.lua")
        .eval()
        .map_err(|source| ConfigWarning::Eval {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(Some(table))
}

/// 合并表 → 强类型 + 合成树:`serde_json` 中转,落型走
/// [`from_tree`](crate::loader::tree::from_tree)(`serde_path_to_error` 拿精确字段路径)。
///
/// # Params:
///   - `table`: 合并后的配置表(函数字段须已摘)
///
/// # Return:
///   `(Config, 合成树)`,转树或落型失败时返回对应告警
fn from_lua_table(table: Table) -> std::result::Result<(Config, serde_json::Value), ConfigWarning> {
    let value = Value::Table(table);
    let json =
        serde_json::to_value(&value).map_err(|source| ConfigWarning::Serialize { source })?;
    let config = crate::loader::tree::from_tree(&json)?;
    Ok((config, json))
}

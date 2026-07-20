//! `load()` 主管线:eval default → eval user → 深合并 → 反序列化,永不因用户配置失败。

use std::path::Path;

use color_eyre::eyre::eyre;
use mlua::{Function, Lua, LuaSerdeExt, Table, Value};

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
pub fn load(user_path: &Path) -> color_eyre::Result<(Config, Vec<ConfigWarning>)> {
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
    install: impl FnOnce(&Lua) -> color_eyre::Result<()>,
) -> color_eyre::Result<DaemonLoad> {
    let lua = Lua::new();
    install(&lua)?;
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
) -> color_eyre::Result<(Config, Vec<ConfigWarning>, bool, serde_json::Value)> {
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
    extract_lua_fns(lua, &merged)?;
    match from_lua_table(merged) {
        Ok((config, tree)) => Ok((config, warnings, true, tree)),
        Err(warning) => {
            warnings.push(warning);
            let (config, tree, warnings) = finalize_default(lua, default_table, warnings)?;
            Ok((config, warnings, false, tree))
        }
    }
}

/// 把配置表里所有 Lua function 字段摘进 VM named registry(serde 落不了型)。
/// **所有 `from_lua_table` 调用点之前都必须过这里**——铁律只锚在此一处,
/// 新增函数字段的提取器挂进来,不要在调用点单独加。
///
/// 各提取器共同语义:非 function 的值不摘——留在表里让落型报 unknown field
/// (带路径),比静默吞掉好定位。配置整体落型失败回落默认时 registry 里可能
/// 残留已摘函数,但默认配置不声明这些字段,无键触达,无害。
fn extract_lua_fns(lua: &Lua, merged: &Table) -> color_eyre::Result<()> {
    extract_copy_templates(lua, merged)?;
    extract_playlist_transforms(lua, merged)?;
    extract_queue_transforms(lua, merged)?;
    Ok(())
}

/// 把 `queue.transforms[i].transform` 从配置表里摘出,按数组序存进 VM named registry
/// (键 [`QUEUE_TRANSFORM_FNS`]);表上的 `transform` 字段移除,`key`/`label` 留下进常规
/// 落型。对位方式与 `tui.copy.templates` 相同(见 [`extract_copy_templates`])。
fn extract_queue_transforms(lua: &Lua, merged: &Table) -> color_eyre::Result<()> {
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
fn extract_copy_templates(lua: &Lua, merged: &Table) -> color_eyre::Result<()> {
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
fn extract_playlist_transforms(lua: &Lua, merged: &Table) -> color_eyre::Result<()> {
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
    pub fn defaults() -> color_eyre::Result<Self> {
        let lua = new_vm()?;
        let table = eval_default(&lua)?;
        extract_lua_fns(&lua, &table)?;
        let (config, _tree) =
            from_lua_table(table).map_err(|w| eyre!("default.lua 无法落成 Config:{w}"))?;
        Ok(config)
    }
}

/// 纯默认配置的合成树(eval `default.lua`,函数字段已摘)。与
/// [`Config::defaults`] 同源;供无真实加载管线的场合(in-proc 调试 / 测试)
/// 当配置宿主的静态底树。
///
/// # Return:
///   默认树;`default.lua` 自身坏(不该发生,有守卫测试)返回 `Err`。
pub fn default_tree() -> color_eyre::Result<serde_json::Value> {
    let lua = new_vm()?;
    let table = eval_default(&lua)?;
    extract_lua_fns(&lua, &table)?;
    let (_config, tree) =
        from_lua_table(table).map_err(|w| eyre!("default.lua 无法落成 Config:{w}"))?;
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
) -> color_eyre::Result<(Config, serde_json::Value, Vec<ConfigWarning>)> {
    extract_lua_fns(lua, &default_table)?;
    let (config, tree) = from_lua_table(default_table)
        .map_err(|w| eyre!("default.lua 无法落成 Config(应被守卫测试拦截):{w}"))?;
    Ok((config, tree, warnings))
}

/// 建 VM 并注入 no-op host stub。
///
/// # Return:
///   就绪的 VM
fn new_vm() -> color_eyre::Result<Lua> {
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
fn eval_default(lua: &Lua) -> color_eyre::Result<Table> {
    let table: Table = lua
        .load(include_str!("../lua/default.lua"))
        .set_name("default.lua")
        .eval()?;
    Ok(table)
}

/// eval 用户文件(若存在)。文件不存在 → `Ok(None)`;eval / 读取失败 → `ConfigWarning::Eval`。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `path`: 用户配置路径
///
/// # Return:
///   `Ok(Some(table))` 用户表 / `Ok(None)` 文件缺失 / `Err(warning)` eval 失败
fn eval_user(lua: &Lua, path: &Path) -> Result<Option<Table>, ConfigWarning> {
    let src = match std::fs::read_to_string(path) {
        Ok(src) => src,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(ConfigWarning::Eval {
                detail: format!("读取 {} 失败:{e}", path.display()),
            });
        }
    };
    let table: Table =
        lua.load(&src)
            .set_name("config.lua")
            .eval()
            .map_err(|e| ConfigWarning::Eval {
                detail: format!("{e}"),
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
///   `(Config, 合成树)`,失败带字段路径
fn from_lua_table(table: Table) -> Result<(Config, serde_json::Value), ConfigWarning> {
    let value = Value::Table(table);
    let json = serde_json::to_value(&value).map_err(|e| ConfigWarning::Deserialize {
        path: String::new(),
        detail: format!("Lua→JSON 转换失败:{e}"),
    })?;
    let config = crate::loader::tree::from_tree(&json)?;
    Ok((config, json))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use color_eyre::eyre::eyre;
    use mineral_model::{BitRate, SearchKind};
    use mlua::{Function, Table};

    use super::load;
    use crate::loader::warning::ConfigWarning;
    use crate::schema::Config;

    /// 把内容写到唯一临时文件,返回路径;调用方测试结束自行删。
    fn temp_config(tag: &str, content: &str) -> color_eyre::Result<PathBuf> {
        let path =
            std::env::temp_dir().join(format!("mineral-cfg-test-{}-{tag}.lua", std::process::id()));
        std::fs::write(&path, content)?;
        Ok(path)
    }

    #[test]
    fn defaults_snapshot() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        mineral_test::assert_snap!("默认配置全量(default.lua → Config)", format!("{cfg:#?}"));
        Ok(())
    }

    /// tui.waveform 默认:进度条波形开、封面取色开(default.lua 是唯一默认值数据源)。
    #[test]
    fn waveform_defaults() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        assert!(*cfg.tui().waveform().enabled(), "波形默认应开启");
        assert!(*cfg.tui().waveform().cover_color(), "封面取色默认应开启");
        assert!(
            (*cfg.tui().waveform().contrast() - 2.0).abs() < f32::EPSILON,
            "对比 gamma 默认 2.0"
        );
        assert_eq!(
            *cfg.tui().waveform().edge_radius(),
            3usize,
            "播放头软边半径默认 3 列"
        );
        Ok(())
    }

    /// audio.envelope 默认:管线粒度(点数 / 块 / 滑窗)与 K-weighting 滤波参数全部
    /// 来自 default.lua(唯一默认值数据源);滤波参数默认值须与 BS.1770 参考实现的
    /// 模拟原型参数逐位一致——48kHz 下推导出规范系数表的正是这组数。
    #[test]
    fn envelope_defaults() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let envelope = cfg.audio().envelope();
        assert_eq!(*envelope.points(), 200usize, "包络定长点数");
        assert_eq!(*envelope.block_ms(), 100u32, "响度块时长");
        assert_eq!(*envelope.window_ms(), 400u32, "momentary 滑窗时长");
        let close = |a: f64, b: f64| (a - b).abs() < 1e-12;
        assert!(close(*envelope.shelf().f0_hz(), 1_681.974_450_955_533));
        assert!(close(*envelope.shelf().gain_db(), 3.999_843_853_973_347));
        assert!(close(*envelope.shelf().q(), 0.707_175_236_955_419_6));
        assert!(close(
            *envelope.shelf().band_exponent(),
            0.499_666_774_154_541_6
        ));
        assert!(close(*envelope.highpass().f0_hz(), 38.135_470_876_024_44));
        assert!(close(*envelope.highpass().q(), 0.500_327_037_323_877_3));
        Ok(())
    }

    /// 用户覆盖 spectrum.style / per-style 子表旋钮:字符串枚举经深合并落型,
    /// 子表覆盖不整表顶掉其余默认。
    #[test]
    fn spectrum_style_override_parses() -> color_eyre::Result<()> {
        let path = temp_config(
            "spectrumstyle",
            r#"return { tui = { spectrum = {
                style = "terrain",
                terrain = { push_ms = 96 },
                bars = { spring_peak = false },
            } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        let spectrum = cfg.tui().spectrum();
        assert_eq!(spectrum.style(), &crate::SpectrumStyle::Terrain);
        assert_eq!(*spectrum.terrain().push_ms(), 96);
        assert_eq!(*spectrum.terrain().layers(), 8, "未覆盖的子表键保默认");
        assert!(!*spectrum.bars().spring_peak());
        assert!(*spectrum.bars().show_trail(), "未覆盖的子表键保默认");
        Ok(())
    }

    /// copy.templates 的函数被摘进 VM registry(下标对位、可调用),
    /// 展示字段(key/label/context)照常落型。
    #[test]
    fn copy_templates_extracted_to_registry() -> color_eyre::Result<()> {
        let path = temp_config(
            "copytpl",
            r#"return { tui = { copy = { templates = {
                { key = "f", label = "Full", template = function(s) return s.title .. "!" end },
                { label = "Pl", context = "playlist", template = function(p) return p.name end },
            } } } }"#,
        )?;
        let loaded = super::load_with_vm(&path, |_lua| Ok(()))?;
        std::fs::remove_file(&path)?;
        let (cfg, warnings, vm) = (loaded.config, loaded.warnings, loaded.vm);
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        let templates = cfg.tui().copy().templates();
        assert_eq!(templates.len(), 2);
        assert_eq!(templates.first().and_then(|t| *t.key()), Some('f'));
        assert_eq!(
            templates.first().map(crate::CopyTemplate::context),
            Some(&crate::CopyContext::Song),
            "context 省略默认 song"
        );
        assert_eq!(
            templates.get(1).map(crate::CopyTemplate::context),
            Some(&crate::CopyContext::Playlist)
        );
        let lua = vm.ok_or_else(|| eyre!("eval 成功必须交还 VM"))?;
        let fns: Table = lua.named_registry_value(super::COPY_TEMPLATE_FNS)?;
        let f1: Function = fns.get(1)?;
        let arg = lua.create_table()?;
        arg.set("title", "Song")?;
        let got: String = f1.call(arg)?;
        assert_eq!(got, "Song!", "registry 函数按 1-based 下标对位且可调用");
        Ok(())
    }

    /// 两级 curate_playlists 函数被摘进各自 registry(per-source 按源名对位、
    /// 跨源独立键),摘除后 sources 段照常落型。
    #[test]
    fn curate_playlists_extracted_to_registry() -> color_eyre::Result<()> {
        let path = temp_config(
            "curate",
            r#"return { sources = {
                bilibili = { curate_playlists = function(lists) return { lists[1] } end },
                curate_playlists = function(all) return all end,
            } }"#,
        )?;
        let loaded = super::load_with_vm(&path, |_lua| Ok(()))?;
        std::fs::remove_file(&path)?;
        let (cfg, warnings, vm) = (loaded.config, loaded.warnings, loaded.vm);
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        assert_eq!(
            *cfg.sources().bilibili().timeout_secs(),
            100,
            "sources 段其余字段仍默认"
        );
        let lua = vm.ok_or_else(|| eyre!("eval 成功必须交还 VM"))?;
        let fns: Table = lua.named_registry_value(super::CURATE_PLAYLISTS_SOURCE_FNS)?;
        let per_source: Function = fns.get("bilibili")?;
        let lists = lua.create_table()?;
        lists.push("a")?;
        lists.push("b")?;
        let kept: Table = per_source.call(lists)?;
        assert_eq!(kept.raw_len(), 1, "per-source 函数按源名对位且可调用");
        let merged: Function = lua.named_registry_value(super::CURATE_PLAYLISTS_MERGED_FN)?;
        let all = lua.create_table()?;
        all.push("x")?;
        let out: Table = merged.call(all)?;
        assert_eq!(out.raw_len(), 1, "跨源函数入独立键且可调用");
        Ok(())
    }

    /// 无配置段的源(如 local)只写 curate_playlists:函数照摘,摘完变空的
    /// 条目一并移除,不触发 unknown field 回落。
    #[test]
    fn curate_playlists_sectionless_entry_removed() -> color_eyre::Result<()> {
        let path = temp_config(
            "curatelocal",
            r#"return { sources = {
                ["local"] = { curate_playlists = function(lists) return lists end },
            } }"#,
        )?;
        let loaded = super::load_with_vm(&path, |_lua| Ok(()))?;
        std::fs::remove_file(&path)?;
        let (cfg, warnings, vm) = (loaded.config, loaded.warnings, loaded.vm);
        assert!(
            warnings.is_empty(),
            "空条目应被移除而非报 unknown field,实得 {warnings:?}"
        );
        assert_eq!(*cfg.audio().volume(), 100);
        let lua = vm.ok_or_else(|| eyre!("eval 成功必须交还 VM"))?;
        let fns: Table = lua.named_registry_value(super::CURATE_PLAYLISTS_SOURCE_FNS)?;
        assert!(
            fns.get::<Function>("local").is_ok(),
            "无段源的函数一样入 registry"
        );
        Ok(())
    }

    /// curate_playlists 不是函数(如字符串):留在表上,落型报 unknown field
    /// 带路径回落默认,不静默吞掉。
    #[test]
    fn curate_playlists_non_function_rejected() -> color_eyre::Result<()> {
        let path = temp_config(
            "curatebad",
            r#"return { sources = { bilibili = { curate_playlists = "keep" } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Deserialize { .. }]),
            "非函数值应报落型告警,实得 {warnings:?}"
        );
        Ok(())
    }

    /// template 不是函数(如字符串):落型报 unknown field(带路径)回落默认,
    /// 不静默吞掉。
    #[test]
    fn copy_template_non_function_rejected() -> color_eyre::Result<()> {
        let path = temp_config(
            "copytplbad",
            r#"return { tui = { copy = { templates = {
                { label = "Bad", template = "{title}" },
            } } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(cfg.tui().copy().templates().is_empty(), "回落默认(空模板)");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Deserialize { .. }]),
            "字符串 template 应报落型告警,实得 {warnings:?}"
        );
        Ok(())
    }

    #[test]
    fn absent_user_file_is_pure_default() -> color_eyre::Result<()> {
        let absent = std::env::temp_dir().join("mineral-cfg-does-not-exist-zzz.lua");
        let (cfg, warnings) = load(&absent)?;
        assert!(warnings.is_empty(), "缺文件不应产 warning");
        assert_eq!(*cfg.audio().volume(), 100);
        Ok(())
    }

    #[test]
    fn user_override_deep_merges() -> color_eyre::Result<()> {
        let path = temp_config("override", "return { audio = { volume = 50 } }")?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty());
        assert_eq!(*cfg.audio().volume(), 50, "覆盖生效");
        assert_eq!(*cfg.download().quality(), BitRate::Lossless, "其余仍默认");
        assert_eq!(*cfg.cache().audio_capacity(), 10 * 1024 * 1024 * 1024);
        Ok(())
    }

    /// 封面缓存预算归 `tui.cover.cache`(disk/image/protocol,client 进程的旋钮),
    /// 顶层 `cache` 只剩 daemon 的 audio_capacity;新路径可独立覆盖,写旧顶层
    /// 字段报 unknown field(带路径)回落默认,提示用户迁移。
    #[test]
    fn cover_cache_lives_under_tui_cover() -> color_eyre::Result<()> {
        let defaults = Config::defaults()?;
        let cache = defaults.tui().cover().cache();
        assert_eq!(*cache.disk(), 4 * 1024 * 1024 * 1024, "磁盘配额默认 4GB");
        assert_eq!(*cache.image(), 128 * 1024 * 1024, "原图 RAM 预算默认 128MB");
        assert_eq!(
            *cache.protocol(),
            64 * 1024 * 1024,
            "协议 RAM 预算默认 64MB"
        );

        // 新路径单字段覆盖:其余两档仍默认(深合并)。
        let path = temp_config(
            "covercache",
            "return { tui = { cover = { cache = { image = 42 } } } }",
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        assert_eq!(*cfg.tui().cover().cache().image(), 42, "覆盖生效");
        assert_eq!(
            *cfg.tui().cover().cache().disk(),
            4 * 1024 * 1024 * 1024,
            "其余档仍默认"
        );

        // 旧顶层路径已迁走:报 unknown field 回落默认,不静默吞。
        let path = temp_config("oldcache", "return { cache = { cover_memory = 1 } }")?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Deserialize { .. }]),
            "旧字段应被拒并提示迁移,实得 {warnings:?}"
        );
        Ok(())
    }

    /// search 白名单:sources / kinds 落型且保配置顺序;默认值的唯一真相在 default.lua
    /// (代码里没有第二份)。
    #[test]
    fn search_whitelists_deserialize_in_order() -> color_eyre::Result<()> {
        let path = temp_config(
            "searchwl",
            r#"return { tui = { search = { channel = {
                sources = { "bilibili", "netease" },
                kinds = { "album", "song" },
            } } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        let channel = cfg.tui().search().channel();
        assert_eq!(
            *channel.sources(),
            vec!["bilibili".to_owned(), "netease".to_owned()],
            "sources 保配置顺序(数组整体替换默认)"
        );
        assert_eq!(
            *channel.kinds(),
            vec![SearchKind::Album, SearchKind::Song],
            "kinds 保配置顺序(数组整体替换默认)"
        );
        // 默认名单的具体内容归 defaults_snapshot 管(default.lua 是唯一真相,这里不复述),
        // 只守「默认必须非空」——空名单会让消费侧走防呆回退,默认态不该踩它。
        let defaults = Config::defaults()?;
        assert!(
            !defaults.tui().search().channel().sources().is_empty(),
            "默认 sources 非空"
        );
        assert!(
            !defaults.tui().search().channel().kinds().is_empty(),
            "默认 kinds 非空"
        );
        Ok(())
    }

    /// kind 是封闭枚举:typo 在加载期报落型告警(带字段路径)并整体回落默认,不静默失效。
    #[test]
    fn search_kind_typo_rejected_with_path() -> color_eyre::Result<()> {
        let path = temp_config(
            "searchwlbad",
            r#"return { tui = { search = { channel = { kinds = { "song", "sogn" } } } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(
            cfg.tui().search().channel().kinds(),
            Config::defaults()?.tui().search().channel().kinds(),
            "回落 default.lua 默认"
        );
        match warnings.as_slice() {
            [ConfigWarning::Deserialize { path, .. }] => {
                assert_eq!(path, "tui.search.channel.kinds[1]", "字段路径应精确到下标");
            }
            other => {
                return Err(eyre!("应有一条 Deserialize warning:{other:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn bad_lua_falls_back_with_eval_warning() -> color_eyre::Result<()> {
        let path = temp_config("badlua", "this is not lua {{{")?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Eval { .. }]),
            "应有一条 Eval warning,实得 {warnings:?}"
        );
        Ok(())
    }

    #[test]
    fn type_error_falls_back_with_field_path() -> color_eyre::Result<()> {
        let path = temp_config("typeerr", r#"return { audio = { volume = "loud" } }"#)?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        match warnings.as_slice() {
            [ConfigWarning::Deserialize { path, .. }] => {
                assert_eq!(path, "audio.volume", "字段路径应精确");
            }
            other => {
                return Err(color_eyre::eyre::eyre!(
                    "应有一条 Deserialize warning:{other:?}"
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn unknown_field_rejected_with_path() -> color_eyre::Result<()> {
        let path = temp_config("unknown", "return { audio = { bogus = 1 } }")?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(*cfg.audio().volume(), 100, "回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Deserialize { .. }]),
            "未知字段应被 deny_unknown_fields 拒,实得 {warnings:?}"
        );
        Ok(())
    }

    #[test]
    fn load_with_vm_keeps_vm_and_live_api_works() -> color_eyre::Result<()> {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU32, Ordering};
        // 活安装回调:挂一个计数函数,模拟 daemon 注入脚本运行时 API。
        let calls = Arc::new(AtomicU32::new(0));
        let calls_in = Arc::clone(&calls);
        let path = temp_config(
            "livevm",
            "mineral.probe()\nreturn { audio = { volume = 60 } }",
        )?;
        let loaded = super::load_with_vm(&path, |lua| {
            let mineral = lua.create_table()?;
            mineral.set(
                "probe",
                lua.create_function(move |_lua, ()| {
                    calls_in.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })?,
            )?;
            lua.globals().set("mineral", mineral)?;
            Ok(())
        })?;
        std::fs::remove_file(&path)?;
        assert!(loaded.warnings.is_empty(), "实得 {:?}", loaded.warnings);
        assert_eq!(*loaded.config.audio().volume(), 60);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "活 API 必须真被调用");
        assert!(loaded.vm.is_some(), "eval 成功必须交还 VM");
        Ok(())
    }

    #[test]
    fn load_with_vm_absent_file_yields_no_vm() -> color_eyre::Result<()> {
        let absent = std::env::temp_dir().join("mineral-cfg-does-not-exist-vm.lua");
        let loaded = super::load_with_vm(&absent, |_lua| Ok(()))?;
        assert!(loaded.warnings.is_empty());
        assert_eq!(*loaded.config.audio().volume(), 100);
        assert!(loaded.vm.is_none(), "无用户文件就无脚本,不交还 VM");
        Ok(())
    }

    #[test]
    fn load_with_vm_eval_failure_sinks_both() -> color_eyre::Result<()> {
        let path = temp_config("badvm", "syntax error {{{")?;
        let loaded = super::load_with_vm(&path, |_lua| Ok(()))?;
        std::fs::remove_file(&path)?;
        assert_eq!(*loaded.config.audio().volume(), 100, "配置回落默认");
        assert!(
            matches!(loaded.warnings.as_slice(), [ConfigWarning::Eval { .. }]),
            "实得 {:?}",
            loaded.warnings
        );
        assert!(loaded.vm.is_none(), "eval 失败配置与脚本同沉,弃 VM");
        assert_eq!(
            loaded.tree.pointer("/audio/volume"),
            Some(&serde_json::json!(100)),
            "失败路径的合成树 = 默认树"
        );
        Ok(())
    }

    /// 合成树暴露:与 Config 同源(用户覆盖已合并)、函数字段已摘不过树、
    /// 树可经 from_tree 再落型出等价 Config。
    #[test]
    fn load_with_vm_exposes_effective_tree() -> color_eyre::Result<()> {
        let path = temp_config(
            "efftree",
            r#"return {
                audio = { volume = 61 },
                tui = { copy = { templates = {
                    { label = "T", template = function(s) return s.title end },
                } } },
            }"#,
        )?;
        let loaded = super::load_with_vm(&path, |_lua| Ok(()))?;
        std::fs::remove_file(&path)?;
        assert!(loaded.warnings.is_empty(), "实得 {:?}", loaded.warnings);
        assert_eq!(
            loaded.tree.pointer("/audio/volume"),
            Some(&serde_json::json!(61)),
            "用户覆盖进树"
        );
        assert_eq!(
            loaded.tree.pointer("/tui/lyrics/fullscreen_line_gap"),
            Some(&serde_json::json!(1)),
            "未覆盖字段是默认值"
        );
        let template_entry = loaded
            .tree
            .pointer("/tui/copy/templates/0")
            .ok_or_else(|| eyre!("templates[0] 应在树里"))?;
        assert!(
            template_entry.get("template").is_none(),
            "函数字段不过树:{template_entry}"
        );
        let retyped =
            crate::loader::tree::from_tree(&loaded.tree).map_err(|w| eyre!("再落型失败:{w}"))?;
        assert_eq!(
            format!("{retyped:?}"),
            format!("{:?}", loaded.config),
            "树与 Config 同源等价"
        );
        Ok(())
    }

    #[test]
    fn top_level_host_call_is_noop() -> color_eyre::Result<()> {
        // 用户顶层调 host API(非 daemon 进程)应安全无错。
        let path = temp_config(
            "hostcall",
            "mineral.on('track_finished', function() end)\nreturn { audio = { volume = 70 } }",
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "no-op stub 应吞调用,实得 {warnings:?}");
        assert_eq!(*cfg.audio().volume(), 70);
        Ok(())
    }

    /// 窗口标题默认配置(全部来自 default.lua):开启、图标四态、有歌模板三段、
    /// idle/disconnected 各两段。
    #[test]
    fn window_title_defaults() -> color_eyre::Result<()> {
        let cfg = Config::defaults()?;
        let wt = cfg.tui().window_title();
        assert!(wt.enabled(), "默认应开启");
        // 图标默认符号。
        assert_eq!(wt.icons().playing(), "⏸");
        assert_eq!(wt.icons().paused(), "▶");
        assert_eq!(wt.icons().idle(), "■");
        assert_eq!(wt.icons().disconnected(), "⚠");
        // 有歌模板:StateIcon + Title + Artist。
        assert!(matches!(
            wt.template().first(),
            Some(crate::TitleSegment::StateIcon { icon: true })
        ));
        assert!(matches!(
            wt.template().get(1),
            Some(crate::TitleSegment::Field {
                field: crate::TitleField::Title,
                ..
            })
        ));
        assert!(matches!(
            wt.template().get(2),
            Some(crate::TitleSegment::Field {
                field: crate::TitleField::Artist,
                ..
            })
        ));
        // idle / disconnected:StateIcon + Literal("Mineral")。
        for tpl in [wt.idle(), wt.disconnected()] {
            assert!(matches!(
                tpl.first(),
                Some(crate::TitleSegment::StateIcon { icon: true })
            ));
            assert!(matches!(
                tpl.get(1),
                Some(crate::TitleSegment::Literal { text }) if text == "Mineral"
            ));
        }
        Ok(())
    }

    /// icons 子表拼错键名应报 unknown field(带路径)回落默认,与全树
    /// deny_unknown_fields 行为一致,不静默吞。
    #[test]
    fn window_title_icons_unknown_key_rejected() -> color_eyre::Result<()> {
        let path = temp_config(
            "wintitleiconbad",
            r#"return { tui = { window_title = { icons = { plying = "▷" } } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert_eq!(wt_icons_playing(&cfg), "⏸", "回落默认");
        assert!(
            matches!(warnings.as_slice(), [ConfigWarning::Deserialize { .. }]),
            "拼错的 icon 键应被拒,实得 {warnings:?}"
        );
        Ok(())
    }

    /// 取窗口标题 playing 图标(测试断言辅助)。
    fn wt_icons_playing(cfg: &Config) -> &str {
        cfg.tui().window_title().icons().playing()
    }

    /// 用户可覆盖 window_title.template 为只含 album 字段的模板。
    #[test]
    fn window_title_user_template() -> color_eyre::Result<()> {
        let path = temp_config(
            "wintitle",
            r#"return { tui = { window_title = { template = { { field = "album" } } } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        let wt = cfg.tui().window_title();
        assert!(wt.enabled(), "未写 enabled 应默认 true");
        assert_eq!(wt.template().len(), 1, "用户覆盖模板长度");
        Ok(())
    }

    /// 用户可完全关闭窗口标题。
    #[test]
    fn window_title_disabled() -> color_eyre::Result<()> {
        let path = temp_config(
            "wintitleoff",
            r#"return { tui = { window_title = { enabled = false } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        assert!(!cfg.tui().window_title().enabled(), "应关闭");
        Ok(())
    }

    /// 新字段端到端:icons 部分覆盖(未覆盖回落默认)、position + pattern 格式、自定义 idle。
    #[test]
    fn window_title_extended_fields() -> color_eyre::Result<()> {
        let path = temp_config(
            "wintitleext",
            r#"return { tui = { window_title = {
                icons = { playing = "▷" },
                template = {
                    { field = "position", format = { pattern = "{m}:{ss}" } },
                    { field = "source" },
                },
                idle = { { text = "睡了" } },
            } } }"#,
        )?;
        let (cfg, warnings) = load(&path)?;
        std::fs::remove_file(&path)?;
        assert!(warnings.is_empty(), "实得 {warnings:?}");
        let wt = cfg.tui().window_title();
        assert_eq!(wt.icons().playing(), "▷", "playing 被覆盖");
        assert_eq!(wt.icons().paused(), "▶", "未覆盖回落默认");
        assert!(
            matches!(
                wt.template().first(),
                Some(crate::TitleSegment::Field {
                    field: crate::TitleField::Position,
                    format: crate::TimeFormat::Pattern { pattern },
                    ..
                }) if pattern == "{m}:{ss}"
            ),
            "position 段带 pattern 格式"
        );
        assert!(matches!(
            wt.template().get(1),
            Some(crate::TitleSegment::Field {
                field: crate::TitleField::Source,
                ..
            })
        ));
        assert_eq!(wt.idle().len(), 1, "idle 覆盖成单段");
        Ok(())
    }
}

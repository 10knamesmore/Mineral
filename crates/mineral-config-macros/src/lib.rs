//! mineral-config 专用 proc-macro:配置 schema 结构体的样板收敛。
//!
//! 独立于 `mineral-macros`(那是普通 lib,导出类型与 `macro_rules!`;
//! `proc-macro = true` 的 crate 只能导出过程宏,两者无法合并)。这里的宏
//! 耦合 mineral-config 的 schema 约定,不做通用件。

mod lua_stub;

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::Nothing;
use syn::{Fields, ItemStruct, parse_macro_input};

/// 配置段三件套:`#[derive(Clone, Debug, Deserialize, Getters)]` +
/// `#[serde(deny_unknown_fields)]` + `#[non_exhaustive]`。
///
/// 统一由宏保证的原因是**防漏**:哪个新段忘了 `deny_unknown_fields`,后果是
/// 静默吞掉用户拼错的键(不报错不回落),且没有守卫测试能在编译期枚举到。
/// 需要额外 derive(如 `Copy`)照常在 struct 上另写 `#[derive(...)]`,两者合并。
///
/// # Params:
///   - `attr`: 无参数(带参报编译错)
///   - `item`: 目标 struct(具名字段)
///
/// # Return:
///   注入三件套后的 struct 定义。
#[proc_macro_attribute]
pub fn config_section(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _no_args = parse_macro_input!(attr as Nothing);
    let item = parse_macro_input!(item as ItemStruct);
    with_section_boilerplate(item)
}

/// 音乐源段:[`macro@config_section`] 三件套 + 注入各源共用的网络字段
/// (`timeout_secs` / `proxy` / `max_connections` / `color`),源特有字段照常
/// 写在 struct 体里(排在共用字段之后)。
///
/// 注入字段引用 `de_proxy` 与 `ColorRef`,**只在 `schema::sources` 模块语境
/// 展开**(两个名字按展开点解析)。
///
/// # Params:
///   - `attr`: 无参数(带参报编译错)
///   - `item`: 目标 struct(具名字段,可为空 `{}`)
///
/// # Return:
///   注入共用字段与三件套后的 struct 定义。
#[proc_macro_attribute]
pub fn source_section(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _no_args = parse_macro_input!(attr as Nothing);
    let mut item = parse_macro_input!(item as ItemStruct);
    let Fields::Named(fields) = &mut item.fields else {
        return syn::Error::new_spanned(&item.ident, "source_section 只支持具名字段 struct")
            .to_compile_error()
            .into();
    };
    let shared: syn::FieldsNamed = syn::parse_quote!({
        /// 单次 API 请求超时(秒)。
        timeout_secs: u64,

        /// 代理:`Some(url)` = 代理地址(如 `socks5://127.0.0.1:1080` / `http://...`);
        /// `None`(Lua `false`)= 不走代理,不接受 `true`。
        #[serde(deserialize_with = "de_proxy")]
        #[lua_type("string|false")]
        proxy: Option<String>,

        /// 到该源的最大并发连接数(`0` = 不限)。
        max_connections: usize,

        /// 来源徽标色:token 名(随主题联动)或 `#rrggbb`(固定品牌色)。
        color: ColorRef,
    });
    let own_fields = std::mem::take(&mut fields.named);
    fields.named = shared.named;
    fields.named.extend(own_fields);
    with_section_boilerplate(item)
}

/// 封闭 serde 枚举的 LuaCATS alias 生成:枚举定义原样保留,另吐
/// `LUA_ALIAS` 关联常量(`---@alias mineral.<Enum> "变体"|...`,变体值按
/// `rename_all` 转换,与落型接受的字符串一致)。要求全单元变体 + 显式
/// `#[serde(rename_all = "lowercase"|"snake_case")]`,不满足报编译错——
/// 带载荷 / untagged 的复合枚举走手写 alias 片段。
///
/// # Params:
///   - `attr`: 无参数(带参报编译错)
///   - `item`: 目标枚举
///
/// # Return:
///   原枚举定义 + `LUA_ALIAS` 关联常量。
#[proc_macro_attribute]
pub fn lua_enum(attr: TokenStream, item: TokenStream) -> TokenStream {
    let _no_args = parse_macro_input!(attr as Nothing);
    let item = parse_macro_input!(item as syn::ItemEnum);
    let alias = match lua_stub::enum_alias(&item) {
        Ok(alias) => alias,
        Err(e) => return e.to_compile_error().into(),
    };
    let ident = &item.ident;
    quote! {
        #item

        impl #ident {
            /// 本枚举的 LuaCATS alias 片段(`lua_enum` 宏从变体定义生成)。
            pub(crate) const LUA_ALIAS: &'static str = #alias;
        }
    }
    .into()
}

/// 给 struct 定义套上三件套并吐出 LuaCATS class 片段常量
/// (两个 attribute 的共同出口)。`lua_*` helper 属性在读取后剥除,
/// 不进最终 struct 定义。
fn with_section_boilerplate(mut item: ItemStruct) -> TokenStream {
    let stub = match lua_stub::class_stub(&item) {
        Ok(stub) => stub,
        Err(e) => return e.to_compile_error().into(),
    };
    lua_stub::strip_lua_attrs(&mut item);
    let ident = &item.ident;
    quote! {
        #[derive(Clone, Debug, serde::Deserialize, derive_getters::Getters)]
        #[serde(deny_unknown_fields)]
        #[non_exhaustive]
        #item

        impl #ident {
            /// 本段的 LuaCATS class 片段(`config_section` 宏从字段定义生成)。
            pub(crate) const LUA_STUB: &'static str = #stub;
        }
    }
    .into()
}

//! LuaCATS stub 片段生成:把 schema struct / enum 的语法树渲染成
//! `meta/config.lua` 里的 `---@class` / `---@alias` 文本。

use syn::{Fields, GenericArgument, ItemStruct, PathArguments, Type};

/// 把 Rust 字段类型映射为 LuaCATS 类型串。
///
/// 规则:标量按固定表;`Option<T>` 剥壳(stub 字段本就全可选);`Vec<T>` 加 `[]`;
/// `FxHashMap<String, T>` 走 `table<string, T>`;其余无泛型的路径类型按约定引用
/// `mineral.<Name>`(定义是否存在由拼装侧闭合性测试兜底)。
///
/// # Params:
///   - `ty`: 字段类型语法树
///
/// # Return:
///   LuaCATS 类型串;无法映射的形态(引用 / 元组 / 未知泛型)返回带 span 的错误
pub(crate) fn map_type(ty: &Type) -> syn::Result<String> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "无法映射为 LuaCATS 类型;用 #[lua_type(\"...\")] 显式标注",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(syn::Error::new_spanned(ty, "空类型路径"));
    };
    let ident = segment.ident.to_string();
    let args = generic_types(&segment.arguments);
    match (ident.as_str(), args.as_slice()) {
        ("bool", []) => Ok("boolean".to_owned()),
        (
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
            | "isize",
            [],
        ) => Ok("integer".to_owned()),
        ("f32" | "f64", []) => Ok("number".to_owned()),
        ("String" | "char" | "PathBuf", []) => Ok("string".to_owned()),
        ("Option", [inner]) => map_type(inner),
        ("Vec", [element]) => Ok(format!("{}[]", map_type(element)?)),
        ("FxHashMap" | "HashMap", [key, value]) => {
            if map_type(key)? != "string" {
                return Err(syn::Error::new_spanned(
                    key,
                    "map 键只支持 string 形态(Lua 表键)",
                ));
            }
            Ok(format!("table<string, {}>", map_type(value)?))
        }
        (_, []) => Ok(format!("mineral.{ident}")),
        (_, _) => Err(syn::Error::new_spanned(
            ty,
            "未知泛型类型;用 #[lua_type(\"...\")] 显式标注",
        )),
    }
}

/// 摘出尖括号泛型里的类型实参(生命周期等非类型实参忽略)。
fn generic_types(arguments: &PathArguments) -> Vec<&Type> {
    let PathArguments::AngleBracketed(bracketed) = arguments else {
        return Vec::new();
    };
    bracketed
        .args
        .iter()
        .filter_map(|arg| match arg {
            GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect()
}

/// 提取 `///` 文档行(剥掉 rustdoc 约定的单个前导空格,保留空行)。
fn doc_lines(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|attr| {
            let syn::Meta::NameValue(nv) = &attr.meta else {
                return None;
            };
            if !nv.path.is_ident("doc") {
                return None;
            }
            let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            else {
                return None;
            };
            let raw = s.value();
            let line = raw.strip_prefix(' ').unwrap_or(&raw);
            // rustdoc 的 intra-doc 链接语法([`x`])对 Lua 读者是噪音,剥成普通反引号内文。
            Some(line.replace("[`", "`").replace("`]", "`"))
        })
        .collect()
}

/// 把字段文档行拼成 `@field` 尾部的单行描述(空行丢弃,行间单空格)。
fn field_doc(attrs: &[syn::Attribute]) -> String {
    doc_lines(attrs)
        .iter()
        .filter(|line| !line.is_empty())
        .cloned()
        .collect::<Vec<String>>()
        .join(" ")
}

/// 字段可选性(`?` 标记)的推导模式。
enum OptionalMode {
    /// 配置段默认:全字段 `?`——过深合并,省略即回落默认,LSP 不该报 missing-fields。
    All,

    /// 数组元素(`#[lua_optional_by_serde]`):不过深合并,可选性按 serde 真实
    /// 语义推导(`Option<T>` 或 `#[serde(default)]` 才 `?`)。
    BySerde,
}

/// 判定属性是否为本宏私有的 `lua_*` helper(须在吐出 struct 前剥除,否则编译器
/// 会当未知属性报错)。
fn is_lua_helper(attr: &syn::Attribute) -> bool {
    ["lua_type", "lua_extra_field", "lua_optional_by_serde"]
        .iter()
        .any(|name| attr.path().is_ident(name))
}

/// 从 struct 与字段上剥除全部 `lua_*` helper 属性(serde 等其他属性保留)。
///
/// # Params:
///   - `item`: 待剥除的 struct 语法树(就地修改)
pub(crate) fn strip_lua_attrs(item: &mut ItemStruct) {
    item.attrs.retain(|attr| !is_lua_helper(attr));
    for field in &mut item.fields {
        field.attrs.retain(|attr| !is_lua_helper(attr));
    }
}

/// 读字段 `#[serde(rename = "...")]` 的重命名值(配置面向用户的字段名以 serde
/// 落型名为准,如 `loop_` → `loop`)。其余 serde 键原样吞掉不解析。
fn serde_rename(attrs: &[syn::Attribute]) -> Option<String> {
    let mut rename = None;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        // serde 属性本身的合法性归 serde derive 校验,这里解析失败静默跳过。
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let value = meta.value()?.parse::<syn::LitStr>()?;
                rename = Some(value.value());
            } else if meta.input.peek(syn::Token![=]) {
                let _ = meta.value()?.parse::<syn::Expr>()?;
            }
            Ok(())
        });
    }
    rename
}

/// 字段是否带 `#[serde(default)]`(含 `default = "path"` 形)。
fn has_serde_default(attrs: &[syn::Attribute]) -> bool {
    let mut found = false;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                found = true;
            }
            if meta.input.peek(syn::Token![=]) {
                let _ = meta.value()?.parse::<syn::Expr>()?;
            }
            Ok(())
        });
    }
    found
}

/// 读字段 `#[lua_type("...")]` 的类型覆盖值。
fn lua_type_override(attrs: &[syn::Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if attr.path().is_ident("lua_type") {
            return Ok(Some(attr.parse_args::<syn::LitStr>()?.value()));
        }
    }
    Ok(None)
}

/// struct 级 `#[lua_extra_field("名[?]", "类型", "描述")]` 声明的追加字段:
/// Rust struct 上没有落点、但用户 config.lua 里真实存在的字段(落型前被摘走的
/// 函数字段)。名字尾缀 `?` 表示可选,原样进 `@field` 行。
struct ExtraField {
    /// 字段名(可带 `?` 尾缀)。
    name: String,

    /// LuaCATS 类型串。
    ty: String,

    /// 单行描述。
    doc: String,
}

/// 解析 struct 上全部 `#[lua_extra_field(...)]`(保声明顺序)。
fn extra_fields(attrs: &[syn::Attribute]) -> syn::Result<Vec<ExtraField>> {
    let mut out = Vec::<ExtraField>::new();
    for attr in attrs {
        if !attr.path().is_ident("lua_extra_field") {
            continue;
        }
        let args = attr.parse_args_with(
            syn::punctuated::Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated,
        )?;
        let mut values = args.iter().map(syn::LitStr::value);
        let (Some(name), Some(ty), Some(doc), None) =
            (values.next(), values.next(), values.next(), values.next())
        else {
            return Err(syn::Error::new_spanned(
                attr,
                "lua_extra_field 需要恰好三个字符串:名[?]、类型、描述",
            ));
        };
        out.push(ExtraField { name, ty, doc });
    }
    Ok(out)
}

/// 字段类型是否为 `Option<...>`。
fn is_option(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Option")
}

/// 渲染一个 schema struct 的 `---@class` 片段:struct 文档逐行 `---`、
/// `---@class mineral.<Struct>`、每字段一行 `---@field <名>[?] <类型> <单行描述>`、
/// 末尾追加 `#[lua_extra_field]` 声明的字段。字段名以 serde 落型名为准
/// (`rename` 生效),类型可被 `#[lua_type]` 覆盖,可选性见 [`OptionalMode`]。
///
/// # Params:
///   - `item`: schema struct 语法树
///
/// # Return:
///   class 片段文本(无尾随换行);具名字段之外的形态或不可映射类型返回错误
pub(crate) fn class_stub(item: &ItemStruct) -> syn::Result<String> {
    let Fields::Named(fields) = &item.fields else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "LuaCATS 生成只支持具名字段 struct",
        ));
    };
    let mode = if item
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("lua_optional_by_serde"))
    {
        OptionalMode::BySerde
    } else {
        OptionalMode::All
    };
    let mut out = Vec::<String>::new();
    for line in doc_lines(&item.attrs) {
        out.push(format!("---{line}"));
    }
    out.push(format!("---@class mineral.{}", item.ident));
    for field in &fields.named {
        let Some(ident) = &field.ident else {
            return Err(syn::Error::new_spanned(field, "字段缺名"));
        };
        let name = serde_rename(&field.attrs).unwrap_or_else(|| ident.to_string());
        let optional = match mode {
            OptionalMode::All => true,
            OptionalMode::BySerde => is_option(&field.ty) || has_serde_default(&field.attrs),
        };
        let marker = if optional { "?" } else { "" };
        let lua_type = match lua_type_override(&field.attrs)? {
            Some(overridden) => overridden,
            None => map_type(&field.ty)?,
        };
        let doc = field_doc(&field.attrs);
        if doc.is_empty() {
            out.push(format!("---@field {name}{marker} {lua_type}"));
        } else {
            out.push(format!("---@field {name}{marker} {lua_type} {doc}"));
        }
    }
    for extra in extra_fields(&item.attrs)? {
        let ExtraField { name, ty, doc } = extra;
        out.push(format!("---@field {name} {ty} {doc}"));
    }
    Ok(out.join("\n"))
}

/// 读 `#[serde(rename_all = "...")]` 的值(struct / enum 级)。
fn serde_rename_all(attrs: &[syn::Attribute]) -> Option<String> {
    let mut rule = None;
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                let value = meta.value()?.parse::<syn::LitStr>()?;
                rule = Some(value.value());
            } else if meta.input.peek(syn::Token![=]) {
                let _ = meta.value()?.parse::<syn::Expr>()?;
            }
            Ok(())
        });
    }
    rule
}

/// 变体名按 serde 规则转小写下划线形(仅支持配置枚举实际用到的两种规则)。
fn apply_rename_all(variant: &str, rule: &str) -> Option<String> {
    match rule {
        "lowercase" => Some(variant.to_lowercase()),
        "snake_case" => {
            let mut out = String::new();
            for (i, ch) in variant.chars().enumerate() {
                if ch.is_uppercase() && i > 0 {
                    out.push('_');
                }
                out.extend(ch.to_lowercase());
            }
            Some(out)
        }
        _ => None,
    }
}

/// 渲染一个封闭 serde 枚举的 `---@alias` 片段:枚举文档逐行 `---`、
/// `---@alias mineral.<Enum> "变体"|...`(变体值按 `rename_all` 规则转换,
/// 与落型接受的字符串一致)。
///
/// 只接受全单元变体 + 显式 `rename_all`(lowercase / snake_case)——带载荷或
/// untagged 的复合枚举形态走手写 alias,缺 `rename_all` 大概率是漏写,直接报错
/// 防止意外产出 PascalCase 值。
///
/// # Params:
///   - `item`: 枚举语法树
///
/// # Return:
///   alias 片段文本(无尾随换行)
pub(crate) fn enum_alias(item: &syn::ItemEnum) -> syn::Result<String> {
    let Some(rule) = serde_rename_all(&item.attrs) else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "lua_enum 要求 #[serde(rename_all = \"lowercase\"|\"snake_case\")]",
        ));
    };
    let mut values = Vec::<String>::new();
    for variant in &item.variants {
        let Fields::Unit = variant.fields else {
            return Err(syn::Error::new_spanned(
                variant,
                "lua_enum 只支持单元变体;复合枚举走手写 alias",
            ));
        };
        let name = variant.ident.to_string();
        let Some(value) = apply_rename_all(&name, &rule) else {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "不支持的 rename_all 规则;只认 lowercase / snake_case",
            ));
        };
        values.push(format!("\"{value}\""));
    }
    let mut out = Vec::<String>::new();
    for line in doc_lines(&item.attrs) {
        out.push(format!("---{line}"));
    }
    out.push(format!(
        "---@alias mineral.{} {}",
        item.ident,
        values.join("|")
    ));
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{class_stub, enum_alias, map_type};

    #[test]
    fn enum_alias_lowercase() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemEnum>(
            r#"
            /// 频谱渲染风格。
            #[serde(rename_all = "lowercase")]
            pub enum SpectrumStyle {
                /// 柱状。
                Bars,
                /// 示波器。
                Scope,
                /// 瀑布。
                Waterfall,
                /// 地形。
                Terrain,
            }
            "#,
        )?;
        let expect = "\
---频谱渲染风格。
---@alias mineral.SpectrumStyle \"bars\"|\"scope\"|\"waterfall\"|\"terrain\"";
        assert_eq!(enum_alias(&item)?, expect);
        Ok(())
    }

    #[test]
    fn enum_alias_snake_case() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemEnum>(
            r#"
            /// 字体效果。
            #[serde(rename_all = "snake_case")]
            pub enum TextStyle {
                /// 加粗。
                Bold,
                /// 删除线。
                CrossedOut,
            }
            "#,
        )?;
        let expect = "\
---字体效果。
---@alias mineral.TextStyle \"bold\"|\"crossed_out\"";
        assert_eq!(enum_alias(&item)?, expect);
        Ok(())
    }

    #[test]
    fn enum_alias_requires_rename_all() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemEnum>(
            r#"
            /// 无 rename_all。
            pub enum Nope {
                /// 变体。
                Foo,
            }
            "#,
        )?;
        assert!(
            enum_alias(&item).is_err(),
            "缺 rename_all 应报错(防意外产出 PascalCase 值)"
        );
        Ok(())
    }

    #[test]
    fn enum_alias_rejects_non_unit_variants() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemEnum>(
            r#"
            /// 复合枚举。
            #[serde(rename_all = "lowercase")]
            pub enum Mixed {
                /// 单元。
                Plain,
                /// 带载荷。
                Pattern(String),
            }
            "#,
        )?;
        assert!(
            enum_alias(&item).is_err(),
            "非单元变体应报错(untagged 复合枚举走手写 alias)"
        );
        Ok(())
    }

    /// 把类型字符串解析成 `syn::Type` 再映射,方便逐条断言。
    fn mapped(ty: &str) -> color_eyre::Result<String> {
        Ok(map_type(&syn::parse_str::<syn::Type>(ty)?)?)
    }

    #[test]
    fn maps_scalar_types() -> color_eyre::Result<()> {
        assert_eq!(mapped("bool")?, "boolean");
        assert_eq!(mapped("u8")?, "integer");
        assert_eq!(mapped("u16")?, "integer");
        assert_eq!(mapped("u32")?, "integer");
        assert_eq!(mapped("u64")?, "integer");
        assert_eq!(mapped("usize")?, "integer");
        assert_eq!(mapped("f32")?, "number");
        assert_eq!(mapped("f64")?, "number");
        assert_eq!(mapped("String")?, "string");
        assert_eq!(mapped("char")?, "string");
        assert_eq!(mapped("PathBuf")?, "string");
        Ok(())
    }

    #[test]
    fn maps_wrapper_types() -> color_eyre::Result<()> {
        assert_eq!(mapped("Option<String>")?, "string");
        assert_eq!(mapped("Option<PathBuf>")?, "string");
        assert_eq!(mapped("Option<char>")?, "string");
        assert_eq!(mapped("Vec<String>")?, "string[]");
        assert_eq!(mapped("Vec<SearchKind>")?, "mineral.SearchKind[]");
        assert_eq!(
            mapped("FxHashMap<String, KeyBinding>")?,
            "table<string, mineral.KeyBinding>"
        );
        assert_eq!(mapped("FxHashMap<String, bool>")?, "table<string, boolean>");
        Ok(())
    }

    #[test]
    fn maps_custom_type_to_namespaced_name() -> color_eyre::Result<()> {
        assert_eq!(mapped("ColorRef")?, "mineral.ColorRef");
        assert_eq!(mapped("EnvelopeConfig")?, "mineral.EnvelopeConfig");
        Ok(())
    }

    #[test]
    fn rejects_unsupported_type_shape() -> color_eyre::Result<()> {
        assert!(
            map_type(&syn::parse_str::<syn::Type>("&'static str")?).is_err(),
            "引用类型应报错"
        );
        assert!(
            map_type(&syn::parse_str::<syn::Type>("(u8, u8)")?).is_err(),
            "元组类型应报错"
        );
        Ok(())
    }

    #[test]
    fn honors_serde_rename() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 跑马灯。
            pub struct MarqueeConfig {
                /// loop 模式子段。
                #[serde(rename = "loop")]
                loop_: MarqueeLoopConfig,
            }
            "#,
        )?;
        let got = class_stub(&item)?;
        assert!(
            got.contains("---@field loop? mineral.MarqueeLoopConfig loop 模式子段。"),
            "rename 后的字段名应进 stub,实得:\n{got}"
        );
        assert!(!got.contains("loop_"), "Rust 侧原名不应出现:\n{got}");
        Ok(())
    }

    #[test]
    fn lua_type_attr_overrides_mapping() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 白名单。
            pub struct ChannelSearchConfig {
                /// source 白名单。
                #[lua_type("mineral.SourceName[]")]
                sources: Vec<String>,
            }
            "#,
        )?;
        let got = class_stub(&item)?;
        assert!(
            got.contains("---@field sources? mineral.SourceName[] source 白名单。"),
            "覆盖类型应生效,实得:\n{got}"
        );
        Ok(())
    }

    #[test]
    fn extra_fields_appended_after_real_fields() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 源段。
            #[lua_extra_field("curate_playlists?", "mineral.CuratePlaylistsFn", "该源歌单策展")]
            pub struct NeteaseSection {
                /// 超时秒。
                timeout_secs: u64,
            }
            "#,
        )?;
        let got = class_stub(&item)?;
        let expect = "\
---源段。
---@class mineral.NeteaseSection
---@field timeout_secs? integer 超时秒。
---@field curate_playlists? mineral.CuratePlaylistsFn 该源歌单策展";
        assert_eq!(got, expect);
        Ok(())
    }

    #[test]
    fn extra_field_without_question_mark_is_required() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 模板。
            #[lua_extra_field("template", "fun(e: mineral.Song): string", "渲染函数")]
            pub struct CopyTemplate {
                /// 显示名。
                label: String,
            }
            "#,
        )?;
        let got = class_stub(&item)?;
        assert!(
            got.contains("---@field template fun(e: mineral.Song): string 渲染函数"),
            "无 ? 后缀的 extra field 应为必填,实得:\n{got}"
        );
        Ok(())
    }

    #[test]
    fn optional_by_serde_marks_only_serde_optional_fields() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 数组元素:不过深合并,可选性按 serde 真实语义。
            #[lua_optional_by_serde]
            pub struct CopyTemplate {
                /// 快捷字母。
                #[serde(default)]
                key: Option<char>,

                /// 显示名。
                label: String,

                /// 上下文。
                #[serde(default)]
                context: CopyContext,
            }
            "#,
        )?;
        let got = class_stub(&item)?;
        assert!(
            got.contains("---@field key? string 快捷字母。"),
            "Option + default 字段应可选:\n{got}"
        );
        assert!(
            got.contains("---@field label string 显示名。"),
            "无 default 非 Option 字段应必填:\n{got}"
        );
        assert!(
            got.contains("---@field context? mineral.CopyContext 上下文。"),
            "serde(default) 字段应可选:\n{got}"
        );
        Ok(())
    }

    #[test]
    fn strip_removes_lua_helper_attrs_keeps_serde() -> color_eyre::Result<()> {
        let mut item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 段。
            #[lua_optional_by_serde]
            #[lua_extra_field("f", "boolean", "x")]
            pub struct S {
                /// 字段。
                #[lua_type("integer")]
                #[serde(rename = "loop")]
                a: u8,
            }
            "#,
        )?;
        super::strip_lua_attrs(&mut item);
        assert!(
            !item.attrs.iter().any(is_lua_attr),
            "struct 级 lua_* 属性应被剥除"
        );
        let field_attrs = item
            .fields
            .iter()
            .flat_map(|f| f.attrs.iter())
            .collect::<Vec<_>>();
        assert!(
            !field_attrs.iter().any(|a| is_lua_attr(a)),
            "字段级 lua_* 属性应被剥除"
        );
        assert!(
            field_attrs.iter().any(|a| a.path().is_ident("serde")),
            "serde 属性应保留"
        );
        Ok(())
    }

    /// 判定属性是否为本宏私有的 `lua_*` helper。
    fn is_lua_attr(attr: &syn::Attribute) -> bool {
        ["lua_type", "lua_extra_field", "lua_optional_by_serde"]
            .iter()
            .any(|name| attr.path().is_ident(name))
    }

    #[test]
    fn strips_intra_doc_links() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 段,见 [`crate::load`]。
            pub struct S {
                /// 写法同 [`ColorValue`],详见 [`Self::defaults`]。
                a: u8,
            }
            "#,
        )?;
        let got = class_stub(&item)?;
        assert!(
            got.contains("---段,见 `crate::load`。"),
            "class 文档的 rustdoc 链接应剥成反引号内文:\n{got}"
        );
        assert!(
            got.contains("---@field a? integer 写法同 `ColorValue`,详见 `Self::defaults`。"),
            "字段文档的 rustdoc 链接应剥成反引号内文:\n{got}"
        );
        Ok(())
    }

    #[test]
    fn class_stub_renders_docs_and_fields() -> color_eyre::Result<()> {
        let item = syn::parse_str::<syn::ItemStruct>(
            r#"
            /// 音频引擎段。
            ///
            /// 改后需重启 daemon。
            pub struct AudioConfig {
                /// 初始音量百分比 0-100。
                volume: u8,

                /// 引擎内参子段,
                /// 慎动。
                envelope: EnvelopeConfig,
            }
            "#,
        )?;
        let expect = "\
---音频引擎段。
---
---改后需重启 daemon。
---@class mineral.AudioConfig
---@field volume? integer 初始音量百分比 0-100。
---@field envelope? mineral.EnvelopeConfig 引擎内参子段, 慎动。";
        assert_eq!(class_stub(&item)?, expect);
        Ok(())
    }
}

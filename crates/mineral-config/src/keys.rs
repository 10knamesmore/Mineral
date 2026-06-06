//! 归一化按键和弦:config 键字符串与语义键的单一表示。
//!
//! 自有表示,不绑定任何输入后端:这里只建模**键盘语义**(字符 / 方向 / Enter 等),
//! 不含任何 UI 框架类型。每个有键盘输入的 client(终端 / 浏览器 / 编辑器宿主)各自
//! 负责把自家原生按键事件归一到 [`KeyChord`];config 字符串(nvim 表示法:
//! `"<Space>"` / `"<S-Left>"`)经 [`KeyChord::parse`] 落到同一表示,两侧在此汇合。
//! 无键盘形态的 client 与本模块无关。

use color_eyre::eyre::bail;

/// 语义键:字符键 + keymap 关心的少量非字符键。
///
/// 字符键大小写有别(`'J'` 即 Shift+j),SHIFT 已编码进字符本身,见 [`KeyChord`] 的不变量。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// 字符键(含空格;大小写有别)。
    Char(char),

    /// 方向键 ←。
    Left,

    /// 方向键 →。
    Right,

    /// 方向键 ↑。
    Up,

    /// 方向键 ↓。
    Down,

    /// Tab。
    Tab,

    /// Enter / 回车。
    Enter,

    /// Esc。
    Esc,

    /// Backspace / 退格。
    Backspace,
}

/// 归一化按键和弦:[`Key`] + 关心的修饰键(仅 SHIFT / CONTROL,其余视为终端噪声)。
///
/// 不变量:**`Key::Char` 永不携带 SHIFT**——大小写 / 符号位形已编码进字符本身
/// (`'J'`、`'+'`),构造与解析两侧都按此归一。派生 `Eq`/`Hash`,可直接当查表 key。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// 语义键。
    key: Key,

    /// SHIFT 修饰(仅对非字符键有意义,见类型级不变量)。
    shift: bool,

    /// CONTROL 修饰。
    ctrl: bool,
}

impl KeyChord {
    /// 无修饰和弦。
    ///
    /// # Params:
    ///   - `key`: 语义键
    ///
    /// # Return:
    ///   无 SHIFT / CONTROL 的和弦
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            shift: false,
            ctrl: false,
        }
    }

    /// 带 SHIFT 的和弦,按类型不变量归一:字母字符键转大写并丢弃 SHIFT,
    /// 其余字符键直接丢弃 SHIFT(位形由字符本身表达),非字符键保留 SHIFT。
    ///
    /// # Params:
    ///   - `key`: 语义键
    ///
    /// # Return:
    ///   归一后的和弦
    pub fn shifted(key: Key) -> Self {
        match key {
            Key::Char(c) => Self::plain(Key::Char(if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase()
            } else {
                c
            })),
            _ => Self {
                key,
                shift: true,
                ctrl: false,
            },
        }
    }

    /// 带 CONTROL 的和弦。
    ///
    /// # Params:
    ///   - `key`: 语义键
    ///
    /// # Return:
    ///   带 CONTROL、无 SHIFT 的和弦
    pub fn ctrl(key: Key) -> Self {
        Self {
            key,
            shift: false,
            ctrl: true,
        }
    }

    /// 在现有和弦上追加 CONTROL 修饰(供事件侧归一时与 SHIFT 组合)。
    ///
    /// # Return:
    ///   带 CONTROL 的同键和弦
    #[must_use]
    pub fn with_ctrl(self) -> Self {
        Self { ctrl: true, ..self }
    }

    /// 解析键字符串(**nvim 表示法**):
    /// - 单字符原样(大小写有别):`j` / `G` / `/` / `+`
    /// - 特殊键与修饰一律尖括号:`<Space>` / `<CR>` / `<Esc>` / `<Tab>` / `<BS>` /
    ///   `<Left>` 等;修饰 `C-`(Ctrl)/ `S-`(Shift),可组合(`<C-S-Left>`)。
    ///   键名与修饰字母大小写不敏感,`<CR>` 有别名 `<Enter>` / `<Return>`。
    ///
    /// # Params:
    ///   - `s`: 键字符串,如 `"j"` / `"<Space>"` / `"<C-g>"` / `"<S-Left>"`
    ///
    /// # Return:
    ///   归一化和弦;空串 / 未知键名 / 未支持修饰(Alt 等)返回 `Err`
    pub fn parse(s: &str) -> color_eyre::Result<Self> {
        // 裸单字符:原样收(大小写 / 符号位形即语义)。
        let mut chars = s.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c != '<'
        {
            return Ok(Self::plain(Key::Char(c)));
        }
        let Some(inner) = s.strip_prefix('<').and_then(|r| r.strip_suffix('>')) else {
            bail!(
                "无法解析键 `{s}`:单字符直接写(如 `j`),特殊键 / 修饰用 nvim 尖括号(如 `<Space>` / `<C-g>` / `<S-Left>`)"
            );
        };
        // 末段是键名,前面的 `X-` 段是修饰;键名本身可以是 `-`(nvim `<C-->`,
        // 双连字符);单 `-` 结尾(如 `<S->`)是缺键名。
        let (mods, name) = if let Some(stripped) = inner.strip_suffix("--") {
            (stripped, "-")
        } else if inner.ends_with('-') {
            bail!("缺键名:`{s}`");
        } else {
            match inner.rfind('-') {
                Some(idx) => inner.split_at(idx + 1),
                None => ("", inner),
            }
        };
        let mut shift = false;
        let mut ctrl = false;
        for part in mods.split('-').filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "c" => ctrl = true,
                "s" => shift = true,
                "a" | "m" => bail!("Alt/Meta 修饰未支持:`{s}`"),
                _ => bail!("未知修饰 `{part}-`(支持 `C-` / `S-`):`{s}`"),
            }
        }
        let key = parse_key_name(name, s)?;
        let base = if shift {
            Self::shifted(key)
        } else {
            Self::plain(key)
        };
        Ok(Self { ctrl, ..base })
    }
}

impl std::fmt::Display for KeyChord {
    /// 规范字符串形式(nvim 表示法,与 [`KeyChord::parse`] 互逆):
    /// 无修饰字符键原样(空格 `<Space>`),其余 `<[C-][S-]键名>`(`<C-g>` / `<C-S-Left>`)。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.key {
            Key::Char(' ') => "Space".to_owned(),
            Key::Char(c) => c.to_string(),
            Key::Left => "Left".to_owned(),
            Key::Right => "Right".to_owned(),
            Key::Up => "Up".to_owned(),
            Key::Down => "Down".to_owned(),
            Key::Tab => "Tab".to_owned(),
            Key::Enter => "CR".to_owned(),
            Key::Esc => "Esc".to_owned(),
            Key::Backspace => "BS".to_owned(),
        };
        // 无修饰的裸字符键不加尖括号(空格除外:裸空格不可读)。
        let bare = !self.ctrl && !self.shift && matches!(self.key, Key::Char(c) if c != ' ');
        if bare {
            return write!(f, "{name}");
        }
        write!(f, "<")?;
        if self.ctrl {
            write!(f, "C-")?;
        }
        if self.shift {
            write!(f, "S-")?;
        }
        write!(f, "{name}>")
    }
}

/// 解析尖括号内的键名段:单字符(大小写敏感)或具名键(大小写不敏感)。
///
/// # Params:
///   - `part`: 键名段
///   - `whole`: 完整原始输入,只用于报错上下文
///
/// # Return:
///   语义键;空段 / 未知键名返回 `Err`
fn parse_key_name(part: &str, whole: &str) -> color_eyre::Result<Key> {
    let mut chars = part.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Ok(Key::Char(c));
    }
    let key = match part.to_ascii_lowercase().as_str() {
        "space" => Key::Char(' '),
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "tab" => Key::Tab,
        "cr" | "enter" | "return" => Key::Enter,
        "esc" | "escape" => Key::Esc,
        "bs" | "backspace" => Key::Backspace,
        "" => bail!("缺键名:`{whole}`"),
        _ => bail!("未知键名 `<{part}>`:`{whole}`"),
    };
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::{Key, KeyChord};

    #[test]
    fn parse_single_char_keys() -> color_eyre::Result<()> {
        assert_eq!(KeyChord::parse("j")?, KeyChord::plain(Key::Char('j')));
        assert_eq!(KeyChord::parse("G")?, KeyChord::plain(Key::Char('G')));
        assert_eq!(KeyChord::parse("/")?, KeyChord::plain(Key::Char('/')));
        assert_eq!(KeyChord::parse("+")?, KeyChord::plain(Key::Char('+')));
        assert_eq!(KeyChord::parse("-")?, KeyChord::plain(Key::Char('-')));
        Ok(())
    }

    #[test]
    fn parse_named_keys_nvim_style_case_insensitive() -> color_eyre::Result<()> {
        assert_eq!(KeyChord::parse("<Space>")?, KeyChord::plain(Key::Char(' ')));
        assert_eq!(KeyChord::parse("<Left>")?, KeyChord::plain(Key::Left));
        assert_eq!(KeyChord::parse("<right>")?, KeyChord::plain(Key::Right));
        assert_eq!(KeyChord::parse("<Up>")?, KeyChord::plain(Key::Up));
        assert_eq!(KeyChord::parse("<down>")?, KeyChord::plain(Key::Down));
        assert_eq!(KeyChord::parse("<Tab>")?, KeyChord::plain(Key::Tab));
        assert_eq!(KeyChord::parse("<CR>")?, KeyChord::plain(Key::Enter));
        assert_eq!(KeyChord::parse("<cr>")?, KeyChord::plain(Key::Enter));
        assert_eq!(KeyChord::parse("<Enter>")?, KeyChord::plain(Key::Enter));
        assert_eq!(KeyChord::parse("<Return>")?, KeyChord::plain(Key::Enter));
        assert_eq!(KeyChord::parse("<Esc>")?, KeyChord::plain(Key::Esc));
        assert_eq!(KeyChord::parse("<BS>")?, KeyChord::plain(Key::Backspace));
        assert_eq!(
            KeyChord::parse("<Backspace>")?,
            KeyChord::plain(Key::Backspace)
        );
        Ok(())
    }

    #[test]
    fn parse_shift_modifier() -> color_eyre::Result<()> {
        assert_eq!(KeyChord::parse("<S-Left>")?, KeyChord::shifted(Key::Left));
        // 修饰字母大小写不敏感。
        assert_eq!(KeyChord::parse("<s-right>")?, KeyChord::shifted(Key::Right));
        // 字母字符键 + Shift 归一为大写字符、丢弃 SHIFT。
        assert_eq!(KeyChord::parse("<S-j>")?, KeyChord::plain(Key::Char('J')));
        Ok(())
    }

    #[test]
    fn parse_ctrl_modifier() -> color_eyre::Result<()> {
        assert_eq!(KeyChord::parse("<C-c>")?, KeyChord::ctrl(Key::Char('c')));
        assert_eq!(KeyChord::parse("<c-Left>")?, KeyChord::ctrl(Key::Left));
        // 组合修饰(非字符键)。
        assert_eq!(
            KeyChord::parse("<C-S-Left>")?,
            KeyChord::shifted(Key::Left).with_ctrl()
        );
        // 键名本身是 `-`(nvim `<C-->` 写法)。
        assert_eq!(KeyChord::parse("<C-->")?, KeyChord::ctrl(Key::Char('-')));
        Ok(())
    }

    #[test]
    fn parse_rejects_invalid() {
        assert!(KeyChord::parse("").is_err(), "空串应报错");
        assert!(KeyChord::parse("foo").is_err(), "多字符裸键名应报错");
        assert!(KeyChord::parse("<S->").is_err(), "缺键名应报错");
        assert!(KeyChord::parse("<A-x>").is_err(), "Alt 未建模应报错");
        assert!(KeyChord::parse("<foo>").is_err(), "未知键名应报错");
        assert!(KeyChord::parse("<Left").is_err(), "尖括号不闭合应报错");
        assert!(
            KeyChord::parse("Shift+Left").is_err(),
            "旧文法已退役,应报错引导 nvim 写法"
        );
        assert!(KeyChord::parse("space").is_err(), "裸具名键已退役");
    }

    #[test]
    fn with_ctrl_equals_ctrl_constructor() {
        assert_eq!(
            KeyChord::plain(Key::Char('c')).with_ctrl(),
            KeyChord::ctrl(Key::Char('c'))
        );
        // SHIFT 与 CONTROL 可组合(非字符键)。
        assert_ne!(
            KeyChord::shifted(Key::Left).with_ctrl(),
            KeyChord::ctrl(Key::Left)
        );
    }

    #[test]
    fn display_round_trips_through_parse() -> color_eyre::Result<()> {
        for chord in [
            KeyChord::plain(Key::Char('j')),
            KeyChord::plain(Key::Char(' ')),
            KeyChord::plain(Key::Char('+')),
            KeyChord::shifted(Key::Left),
            KeyChord::ctrl(Key::Char('c')),
            KeyChord::shifted(Key::Right).with_ctrl(),
            KeyChord::plain(Key::Esc),
            KeyChord::ctrl(Key::Char('-')),
        ] {
            assert_eq!(
                KeyChord::parse(&chord.to_string())?,
                chord,
                "Display 应与 parse 互逆:{chord}"
            );
        }
        Ok(())
    }

    #[test]
    fn display_uses_nvim_notation() {
        assert_eq!(KeyChord::plain(Key::Char(' ')).to_string(), "<Space>");
        assert_eq!(KeyChord::plain(Key::Enter).to_string(), "<CR>");
        assert_eq!(KeyChord::ctrl(Key::Char('g')).to_string(), "<C-g>");
        assert_eq!(
            KeyChord::shifted(Key::Left).with_ctrl().to_string(),
            "<C-S-Left>"
        );
        assert_eq!(KeyChord::plain(Key::Char('J')).to_string(), "J");
    }

    #[test]
    fn shifted_char_normalizes_to_uppercase() {
        assert_eq!(
            KeyChord::shifted(Key::Char('j')),
            KeyChord::plain(Key::Char('J')),
            "字母 + Shift 应归一为大写、丢 SHIFT 位"
        );
        assert_eq!(
            KeyChord::shifted(Key::Char('+')),
            KeyChord::plain(Key::Char('+')),
            "非字母字符 + Shift 同样丢 SHIFT 位(位形在字符里)"
        );
    }
}

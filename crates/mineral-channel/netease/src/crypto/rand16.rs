use rand::Rng;

use super::constants::STD_CHARS;

/// 生成 16 字节随机 key,以及它的"同字符反序"版本(spec §3.4)。
///
/// 注意两个数组**字符相同、顺序相反**——不是各自独立随机。
pub fn new_len16_rand() -> ([u8; 16], [u8; 16]) {
    let mut rng = rand::rng();
    let mut a = [0u8; 16];
    let mut b = [0u8; 16];
    for i in 0..16 {
        let idx = rng.random_range(0..STD_CHARS.len());
        let c = STD_CHARS[idx];
        a[i] = c;
        b[15 - i] = c;
    }
    (a, b)
}

use num_bigint::BigUint;
use once_cell::sync::Lazy;

use super::constants::RSA_PUBLIC_KEY_PEM;

/// 解析 PEM 拿 (n, e)。手写 PEM → DER → 简单 ASN.1 解码,避免引入 rsa crate。
fn parse_public_key_pem(pem: &str) -> (BigUint, BigUint) {
    // 1. 去掉 PEM header/footer 与换行,base64 解码出 SPKI DER。
    let mut b64 = String::new();
    for line in pem.lines() {
        let line = line.trim();
        if line.starts_with("-----") || line.is_empty() {
            continue;
        }
        b64.push_str(line);
    }
    use base64::Engine;
    let der = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .expect("invalid base64 in RSA pem");

    // 2. SPKI 结构:
    //    SubjectPublicKeyInfo SEQUENCE {
    //       AlgorithmIdentifier,
    //       BIT STRING { RSAPublicKey SEQUENCE { modulus INTEGER, publicExponent INTEGER } }
    //    }
    //    我们手工解析最外 SEQUENCE → skip Algorithm SEQUENCE → 进 BIT STRING →
    //    去 unused-bits 字节 → 解 inner SEQUENCE → 取两个 INTEGER。
    let mut p = Parser::new(&der);
    let outer = p.expect_seq();
    let mut q = Parser::new(outer);
    let _algo = q.expect_seq(); // 跳过 AlgorithmIdentifier
    let bitstring = q.expect_tag(0x03);
    // BIT STRING 第一个字节是 unused bits 数(此处 0)
    let inner = &bitstring[1..];
    let mut r = Parser::new(inner);
    let key_seq = r.expect_seq();
    let mut s = Parser::new(key_seq);
    let n_bytes = s.expect_integer();
    let e_bytes = s.expect_integer();

    (
        BigUint::from_bytes_be(n_bytes),
        BigUint::from_bytes_be(e_bytes),
    )
}

struct Parser<'a> {
    buf: &'a [u8],
}

impl<'a> Parser<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    fn read_byte(&mut self) -> u8 {
        let b = self.buf[0];
        self.buf = &self.buf[1..];
        b
    }

    fn read_len(&mut self) -> usize {
        let first = self.read_byte();
        if first & 0x80 == 0 {
            first as usize
        } else {
            let n = (first & 0x7F) as usize;
            let mut len = 0usize;
            for _ in 0..n {
                len = (len << 8) | self.read_byte() as usize;
            }
            len
        }
    }

    fn expect_tag(&mut self, tag: u8) -> &'a [u8] {
        let t = self.read_byte();
        assert_eq!(t, tag, "expected tag {tag:#04x}, got {t:#04x}");
        let len = self.read_len();
        let (head, rest) = self.buf.split_at(len);
        self.buf = rest;
        head
    }

    fn expect_seq(&mut self) -> &'a [u8] {
        self.expect_tag(0x30)
    }

    fn expect_integer(&mut self) -> &'a [u8] {
        let body = self.expect_tag(0x02);
        // 去掉前导 0x00(为表达正数避免被误认为负数)
        if !body.is_empty() && body[0] == 0 {
            &body[1..]
        } else {
            body
        }
    }
}

static N_AND_E: Lazy<(BigUint, BigUint)> = Lazy::new(|| parse_public_key_pem(RSA_PUBLIC_KEY_PEM));

/// 网易云风格的"无 padding RSA":
///
/// 1. 把 16 字节 secret_key 左侧补 112 个 0x00 → 128 字节。
/// 2. 当作大整数 m,做 m^e mod n。
/// 3. 把结果转回 big-endian 字节,**左 pad 到 128 字节**。
pub fn rsa_no_padding_encrypt(secret_key: &[u8; 16]) -> Vec<u8> {
    let (n, e) = &*N_AND_E;
    let mut buf = vec![0u8; 128 - 16];
    buf.extend_from_slice(secret_key);
    let m = BigUint::from_bytes_be(&buf);
    let c = m.modpow(e, n);
    let bytes = c.to_bytes_be();
    if bytes.len() == 128 {
        bytes
    } else {
        let mut out = vec![0u8; 128 - bytes.len()];
        out.extend(bytes);
        out
    }
}

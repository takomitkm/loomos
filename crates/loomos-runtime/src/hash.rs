//! SHA-256（FIPS 180-4）——仓库级完整性哈希基础设施（组件包校验和 / A/B bundle
//! 工件校验共用；放在 runtime 因为它是与权威语义无关的纯工具层）。
//!
//! **为什么自实现而不引 sha2 crate**：dependency-ledger 纪律（architecture/08
//! §8.2–8.3）要求每个新依赖过准入。纯标准库实现 + NIST 官方向量 + hashlib
//! 对齐向量锁定，零传递依赖，比引入 sha2 更可审计。
//! 不用于密码学抗碰撞场景（那是签名密钥的职责，见 ADR-013）。

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// 增量式 SHA-256 状态（大文件/流式）。
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        // padding: 0x80 + zeros + 8-byte bit length（len(+1) mod 64 ≡ 56）
        let mut pad = vec![0x80u8];
        let rem = (self.total_len.wrapping_add(1)) % 64;
        let zeros = (if rem <= 56 { 56 - rem } else { 120 - rem }) as usize;
        pad.resize(pad.len() + zeros, 0u8);
        pad.extend_from_slice(&bit_len.to_be_bytes());
        self.update(&pad);
        // 消息长度恰为 64 的倍数时，pad 是独立整块且 update 后 buf 空；
        // 若 pad 落在尾缓冲（buf_len 1..=63），必须补零压缩尾块
        if self.buf_len > 0 {
            let mut block = self.buf;
            block[self.buf_len..].fill(0);
            self.compress(&block);
            self.buf_len = 0;
        }
        let mut out = [0u8; 32];
        for (i, w) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        let round = [a, b, c, d, e, f, g, hh];
        for (i, r) in round.iter().enumerate() {
            self.h[i] = self.h[i].wrapping_add(*r);
        }
    }
}

/// 一次性哈希（便捷入口）。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

fn hex(digest: &[u8]) -> String {
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NIST FIPS 180-4 官方测试向量。
    #[test]
    fn nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn million_a_matches_reference() {
        // 一百万个 'a'。注意：部分网络资料把尾段写成 047d39dccad7f931（讹传）；
        // 本仓库以两个独立实现交叉验证的值为准（python hashlib 3.11 + .NET
        // SHA256CryptoServiceProvider 输出一致）。
        let data = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex(&data),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// 64 倍数长度回归（对照 hashlib；曾暴露 finalize 尾块未压缩缺陷）。
    #[test]
    fn multiples_of_block_match_reference() {
        let cases: &[(usize, &str)] = &[
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            (
                192,
                "7cee24628d290c16183532716cc5a8a889bc951b4b0a1507c32b8e29cee01052",
            ),
            (
                320,
                "fee1a848cb6763c3088a0e523541b46ac171af2677b062178a9116c4612b352f",
            ),
            (
                640,
                "9a6aad64bae7522d8b6cbde674c6268563dd27f74f4fe5b55c66942b744c7399",
            ),
        ];
        for &(n, want) in cases {
            assert_eq!(sha256_hex(&vec![b'a'; n]), want, "长度 {n}");
        }
    }

    /// padding 边界（对照 hashlib 基准）：55/56/63/64/65/127/128 + 多块内容。
    #[test]
    fn padding_boundaries_match_reference() {
        let cases: &[(usize, u8, &str)] = &[
            (55, b'a', "9f4390f8d30c2dd9"),
            (56, b'a', "b35439a4ac6f0948"),
            (63, b'a', "7d3e74a05d7db15b"),
            (64, b'a', "ffe054fe7ae0cb6d"),
            (65, b'a', "635361c48bb9eab1"),
            (127, b'a', "c57e9278af78fa3c"),
            (128, b'a', "6836cf13bac400e9"),
        ];
        for &(n, fill, want) in cases {
            let got = sha256_hex(&vec![fill; n]);
            assert_eq!(&got[..16], want, "长度 {n} 的 padding 边界错误");
        }
        assert_eq!(
            &sha256_hex(&[b'b'; 200])[..16],
            "aaebc35c4c4e2cc7",
            "200 字节多块错误"
        );
    }

    #[test]
    fn incremental_matches_one_shot() {
        let data: Vec<u8> = (0..500u32).map(|i| (i * 7 % 256) as u8).collect();
        let want = sha256_hex(&data);
        for chunk in [1usize, 3, 63, 64, 65, 128, 250] {
            let mut h = Sha256::new();
            for part in data.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(hex(&h.finalize()), want, "分片 {chunk} 与一次性哈希不一致");
        }
    }
}

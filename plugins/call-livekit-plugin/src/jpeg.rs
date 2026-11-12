//! 极简基线 JPEG 编码（4:4:4，无子采样）。
//!
//! 视频帧转图片只是为了让视觉模型「看一眼」，不需要高压缩率；
//! 用 4:4:4 基线编码能避开色度子采样和渐进式 DCT 的复杂度。

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const STD_LUMA_Q: [u8; 64] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

const STD_CHROMA_Q: [u8; 64] = [
    17, 18, 24, 47, 99, 99, 99, 99, 18, 21, 26, 66, 99, 99, 99, 99, 24, 26, 56, 99, 99, 99, 99, 99,
    47, 66, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
    99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99,
];

const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALUES: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALUES: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
    0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
    0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALUES: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
    0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
    0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
    0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
    0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
    0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
    0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
    0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
    0xf9, 0xfa,
];

/// 规范化哈夫曼表 → 码表（长度 ≤ 16 的位串）。
struct HuffTable {
    codes: [(u16, u8); 256],
}

impl HuffTable {
    fn new(bits: &[u8; 16], values: &[u8]) -> Self {
        let mut codes = [(0u16, 0u8); 256];
        let mut code = 0u16;
        let mut idx = 0usize;
        for (len_idx, &count) in bits.iter().enumerate() {
            let len = (len_idx + 1) as u8;
            for _ in 0..count {
                codes[values[idx] as usize] = (code, len);
                code += 1;
                idx += 1;
            }
            code <<= 1;
        }
        Self { codes }
    }

    fn code(&self, symbol: u8) -> (u16, u8) {
        self.codes[symbol as usize]
    }
}

struct BitWriter {
    out: Vec<u8>,
    acc: u32,
    nbits: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            acc: 0,
            nbits: 0,
        }
    }

    fn write_bits(&mut self, value: u16, len: u8) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (value as u32 & ((1u32 << len) - 1));
        self.nbits += len;
        while self.nbits >= 8 {
            let shift = self.nbits - 8;
            let byte = ((self.acc >> shift) & 0xff) as u8;
            self.out.push(byte);
            if byte == 0xff {
                // 熵编码数据里 0xFF 必须转义成 0xFF 0x00。
                self.out.push(0x00);
            }
            self.nbits -= 8;
            self.acc &= (1u32 << self.nbits) - 1;
        }
    }

    fn flush(&mut self) {
        if self.nbits > 0 {
            let pad = 8 - self.nbits;
            self.write_bits((1u16 << pad) - 1, pad);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Comp {
    Luma,
    Chroma,
}

/// IJG 质量缩放：quality 50 → 标准表，100 → 全 1。
fn scaled_table(base: &[u8; 64], quality: u8) -> [u8; 64] {
    let quality = quality.clamp(1, 100) as u32;
    let factor = if quality < 50 {
        5000 / quality
    } else {
        200 - quality * 2
    };
    let mut table = [0u8; 64];
    for i in 0..64 {
        table[i] = (((base[i] as u32 * factor + 50) / 100).clamp(1, 255)) as u8;
    }
    table
}

/// DQT 段按 zigzag 顺序存放量化表。
fn zigzag(table: &[u8; 64]) -> [u8; 64] {
    let mut out = [0u8; 64];
    for (i, &pos) in ZIGZAG.iter().enumerate() {
        out[i] = table[pos];
    }
    out
}

struct Encoder {
    dc: HuffTable,
    ac: HuffTable,
    q: [i32; 64],
    prev_dc: i32,
}

impl Encoder {
    fn new(comp: Comp, quality: u8) -> Self {
        let base: &[u8; 64] = match comp {
            Comp::Luma => &STD_LUMA_Q,
            Comp::Chroma => &STD_CHROMA_Q,
        };
        let table = scaled_table(base, quality);
        let mut q = [0i32; 64];
        for i in 0..64 {
            q[i] = table[i] as i32;
        }
        let (dc, ac) = match comp {
            Comp::Luma => (
                HuffTable::new(&DC_LUMA_BITS, &DC_VALUES),
                HuffTable::new(&AC_LUMA_BITS, &AC_LUMA_VALUES),
            ),
            Comp::Chroma => (
                HuffTable::new(&DC_CHROMA_BITS, &DC_VALUES),
                HuffTable::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES),
            ),
        };
        Self {
            dc,
            ac,
            q,
            prev_dc: 0,
        }
    }

    fn encode_block(&mut self, w: &mut BitWriter, block: &[f32; 64]) {
        let mut dct = [0f32; 64];
        fdct(block, &mut dct);
        let mut coeffs = [0i32; 64];
        for i in 0..64 {
            coeffs[i] = (dct[ZIGZAG[i]] / self.q[i] as f32).round() as i32;
        }
        // DC
        let diff = coeffs[0] - self.prev_dc;
        self.prev_dc = coeffs[0];
        Self::write_number(w, diff, &self.dc);
        // AC
        let mut run = 0u8;
        for &c in &coeffs[1..] {
            if c == 0 {
                run += 1;
            } else {
                while run > 15 {
                    let (code, len) = self.ac.code(0xf0);
                    w.write_bits(code, len);
                    run -= 16;
                }
                let size = bit_size(c);
                let (code, len) = self.ac.code(((run << 4) | size as u8) & 0xff);
                w.write_bits(code, len);
                write_value(w, c, size);
                run = 0;
            }
        }
        if run > 0 {
            let (code, len) = self.ac.code(0x00);
            w.write_bits(code, len);
        }
    }

    fn write_number(w: &mut BitWriter, value: i32, table: &HuffTable) {
        let size = bit_size(value);
        let (code, len) = table.code(size as u8);
        w.write_bits(code, len);
        write_value(w, value, size);
    }
}

fn bit_size(v: i32) -> u32 {
    let mut a = v.unsigned_abs();
    let mut n = 0;
    while a > 0 {
        n += 1;
        a >>= 1;
    }
    n
}

fn write_value(w: &mut BitWriter, v: i32, size: u32) {
    if size == 0 {
        return;
    }
    let bits = if v < 0 { v - 1 + (1 << size) } else { v } as u32;
    w.write_bits(bits as u16, size as u8);
}

fn fdct(input: &[f32; 64], out: &mut [f32; 64]) {
    // 分离式 8x8 DCT-II：out[v][u] = 1/4·C(u)C(v)·ΣΣ x[y][x]·cos((2y+1)uπ/16)·cos((2x+1)vπ/16)
    let mut tmp = [0f32; 64];
    for v in 0..8 {
        for x in 0..8 {
            let mut sum = 0f32;
            for y in 0..8 {
                // COS 表是 1-D 行；二维系数用 COS[v*8+y]（u 或 v 的振荡方向）。
                sum += input[y * 8 + x] * COS[v * 8 + y];
            }
            tmp[v * 8 + x] = sum;
        }
    }
    for v in 0..8 {
        for u in 0..8 {
            let mut sum = 0f32;
            for x in 0..8 {
                sum += tmp[v * 8 + x] * COS[u * 8 + x];
            }
            // JPEG 正变换归一化 C(u)·C(v)/4，C(0)=1/√2、其余为 1。
            out[v * 8 + u] = sum * 0.25 * C_UV[u] * C_UV[v];
        }
    }
}

/// JPEG 归一化因子 C(k)：k=0 时为 1/√2，其余为 1。
const C_UV: [f32; 8] = [0.707_106_77, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];

fn cos_table() -> [f32; 64] {
    let mut t = [0f32; 64];
    for (u, row) in t.chunks_exact_mut(8).enumerate() {
        for (x, v) in row.iter_mut().enumerate() {
            *v = (((2 * x + 1) as f32) * (u as f32) * std::f32::consts::PI / 16.0).cos();
        }
    }
    t
}

static COS: std::sync::LazyLock<[f32; 64]> = std::sync::LazyLock::new(cos_table);

fn rgb_to_ycbcr(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    (
        0.299 * r + 0.587 * g + 0.114 * b - 128.0,
        -0.168_736 * r - 0.331_264 * g + 0.5 * b,
        0.5 * r - 0.418_688 * g - 0.081_312 * b,
    )
}

/// 编码 RGB（每像素 3 字节，行优先）为基线 JPEG。
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8], quality: u8) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    // 头部固定 609 字节（两条 DQT、SOF0、四张 DHT、SOS）外加 SOS 数据。
    let mut out = Vec::with_capacity(640 + rgb.len() / 8);
    write_headers(&mut out, width, height, quality);

    // 先把整幅图转成 Y/Cb/Cr 平面（块按行优先交错编码，Cb/Cr 每块重复一遍 Y 的像素偏移）。
    let mut y_plane = vec![0f32; width * height];
    let mut cb_plane = vec![0f32; width * height];
    let mut cr_plane = vec![0f32; width * height];
    for i in 0..width * height {
        let (ly, b, r) = rgb_to_ycbcr(rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]);
        y_plane[i] = ly;
        cb_plane[i] = b;
        cr_plane[i] = r;
    }

    let mut luma = Encoder::new(Comp::Luma, quality);
    let mut cb = Encoder::new(Comp::Chroma, quality);
    let mut cr = Encoder::new(Comp::Chroma, quality);
    let mut w = BitWriter::new();
    let mut block_y = [0f32; 64];
    let mut block_cb = [0f32; 64];
    let mut block_cr = [0f32; 64];

    for by in (0..height).step_by(8) {
        for bx in (0..width).step_by(8) {
            for i in 0..8 {
                for j in 0..8 {
                    let x = (bx + j).min(width - 1);
                    let y = (by + i).min(height - 1);
                    let idx = y * width + x;
                    block_y[i * 8 + j] = y_plane[idx];
                    block_cb[i * 8 + j] = cb_plane[idx];
                    block_cr[i * 8 + j] = cr_plane[idx];
                }
            }
            luma.encode_block(&mut w, &block_y);
            cb.encode_block(&mut w, &block_cb);
            cr.encode_block(&mut w, &block_cr);
        }
    }
    w.flush();
    out.extend_from_slice(&w.out);
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

fn write_headers(out: &mut Vec<u8>, width: usize, height: usize, quality: u8) {
    // SOI
    out.extend_from_slice(&[0xff, 0xd8]);
    // APP0 JFIF
    out.extend_from_slice(&[
        0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0,
    ]);
    // DQT（与编码时使用的量化表一致，按质量缩放）
    out.extend_from_slice(&[0xff, 0xdb, 0, 67, 0]);
    out.extend_from_slice(&zigzag(&scaled_table(&STD_LUMA_Q, quality)));
    out.extend_from_slice(&[0xff, 0xdb, 0, 67, 1]);
    out.extend_from_slice(&zigzag(&scaled_table(&STD_CHROMA_Q, quality)));
    // SOF0
    out.extend_from_slice(&[0xff, 0xc0, 0, 17, 8]);
    out.extend_from_slice(&(height as u16).to_be_bytes());
    out.extend_from_slice(&(width as u16).to_be_bytes());
    out.extend_from_slice(&[3, 1, 0x11, 0, 2, 0x11, 1, 3, 0x11, 1]);
    // DHT
    write_dht(out, 0x00, &DC_LUMA_BITS, &DC_VALUES);
    write_dht(out, 0x10, &AC_LUMA_BITS, &AC_LUMA_VALUES);
    write_dht(out, 0x01, &DC_CHROMA_BITS, &DC_VALUES);
    write_dht(out, 0x11, &AC_CHROMA_BITS, &AC_CHROMA_VALUES);
    // SOS
    out.extend_from_slice(&[0xff, 0xda, 0, 12, 3, 1, 0x00, 2, 0x11, 3, 0x11, 0, 63, 0]);
}

fn write_dht(out: &mut Vec<u8>, class: u8, bits: &[u8; 16], values: &[u8]) {
    let len = 2 + 1 + 16 + values.len();
    out.extend_from_slice(&[0xff, 0xc4]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.push(class);
    out.extend_from_slice(bits);
    out.extend_from_slice(values);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_valid_jpeg_markers() {
        let w = 32u32;
        let h = 24u32;
        let rgb: Vec<u8> = (0..(w * h))
            .flat_map(|i| [i as u8, 128, 255 - (i % 255) as u8])
            .collect();
        let jpeg = encode_rgb(w, h, &rgb, 70);
        assert_eq!(&jpeg[0..2], &[0xff, 0xd8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xff, 0xd9]);
        assert!(jpeg.len() > 200);
    }

    /// 色块图解码回来必须接近原色（曾经因 DQT 写成自然顺序导致整图偏色）。
    /// 自己按文件里的 DQT/DHT 反解出 YCbCr，再转回 RGB 与原图逐像素比较。
    #[test]
    fn solid_color_decodes_close_to_source() {
        let (w, h) = (16u32, 16u32);
        for src in [[200u8, 100, 0], [10, 128, 245]] {
            let rgb: Vec<u8> = (0..(w * h)).flat_map(|_| src).collect();
            let jpeg = encode_rgb(w, h, &rgb, 90);
            let (dw, dh, decoded) = decode_for_test(&jpeg);
            assert_eq!((dw, dh), (w as usize, h as usize));
            for px in detached_pixels(&decoded) {
                for c in 0..3 {
                    let diff = (px[c] as i32 - src[c] as i32).abs();
                    assert!(diff <= 3, "源 {:?} 解出 {:?}，通道 {c} 偏 {diff}", src, px);
                }
            }
        }
    }

    /// 色差不能丢：左半红、右半蓝，两半的色相与亮度都要还原。
    #[test]
    fn chroma_changes_survive_roundtrip() {
        let (w, h) = (16u32, 16u32);
        let left = [220u8, 40, 40];
        let right = [30u8, 60, 220];
        let rgb: Vec<u8> = (0..(w * h))
            .flat_map(|i| if (i as u32) % w < w / 2 { left } else { right })
            .collect();
        let jpeg = encode_rgb(w, h, &rgb, 90);
        let (_, _, decoded) = decode_for_test(&jpeg);
        for y in [0usize, 8] {
            let l = decoded[y * w as usize];
            let r = decoded[y * w as usize + 8];
            for c in 0..3 {
                let dl = (l[c] as i32 - left[c] as i32).abs();
                let dr = (r[c] as i32 - right[c] as i32).abs();
                assert!(
                    dl <= 4 && dr <= 4,
                    "行 {y} 左 {l:?} 右 {r:?}（源 {left:?}/{right:?}）"
                );
            }
        }
    }

    /// 解码结果：4:4:4 块按「Y, Cb, Cr」顺序交错存放。
    fn decode_for_test(jpeg: &[u8]) -> (usize, usize, Vec<[u8; 3]>) {
        let dqt_at = find_marker(jpeg, 0xdb);
        let ql: [u8; 64] = jpeg[dqt_at + 5..dqt_at + 69].try_into().unwrap();
        let dqt2_at = find_marker(&jpeg[dqt_at + 2..], 0xdb) + dqt_at + 2;
        let qc: [u8; 64] = jpeg[dqt2_at + 5..dqt2_at + 69].try_into().unwrap();
        let sof_at = find_marker(jpeg, 0xc0);
        let dh = u16::from_be_bytes([jpeg[sof_at + 5], jpeg[sof_at + 6]]) as usize;
        let dw = u16::from_be_bytes([jpeg[sof_at + 7], jpeg[sof_at + 8]]) as usize;
        let sos_at = find_marker(jpeg, 0xda);
        let sos_len = u16::from_be_bytes([jpeg[sos_at + 2], jpeg[sos_at + 3]]) as usize;
        let mut bits = Vec::new();
        let mut i = sos_at + 2 + sos_len;
        while i + 1 < jpeg.len() {
            let b = jpeg[i];
            bits.extend((0..8).map(|k| (b >> (7 - k)) & 1 == 1));
            i += if b == 0xff { 2 } else { 1 };
        }
        let mut pos = 0usize;
        let mut prev = [0i32; 3];
        let mut planes: [Vec<f32>; 3] = std::array::from_fn(|_| vec![0.0f32; dw * dh]);
        let mut coeffs = [0i32; 64];
        for by in (0..dh).step_by(8) {
            for bx in (0..dw).step_by(8) {
                for c in 0..3 {
                    coeffs.iter_mut().for_each(|v| *v = 0);
                    // 注意别写成 `HuffTable::new(&if c == 0 { .. } else { .. })` 这种
                    // 对整体取引用的形式——表会全部落成同一个。
                    let dc_table = match c {
                        0 => HuffTable::new(&DC_LUMA_BITS, &DC_VALUES),
                        _ => HuffTable::new(&DC_CHROMA_BITS, &DC_VALUES),
                    };
                    let ac_table = match c {
                        0 => HuffTable::new(&AC_LUMA_BITS, &AC_LUMA_VALUES),
                        _ => HuffTable::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES),
                    };
                    let q = if c == 0 { &ql } else { &qc };
                    let size = decode_symbol(&bits, &mut pos, &dc_table) as u32;
                    prev[c] += read_value(&bits, &mut pos, size);
                    coeffs[0] = prev[c] * q[0] as i32;
                    let mut k = 1usize;
                    loop {
                        let sym = decode_symbol(&bits, &mut pos, &ac_table);
                        let run = (sym >> 4) as usize;
                        let size = (sym & 0x0f) as u32;
                        if sym == 0 {
                            break;
                        }
                        k += run;
                        let v = read_value(&bits, &mut pos, size);
                        coeffs[k] = v * q[k] as i32;
                        k += 1;
                    }
                    // EOB（symbol 0）由 decode_symbol 读走，loop 已 break。
                    let mut nat = [0i32; 64];
                    for (zz, &p) in ZIGZAG.iter().enumerate() {
                        nat[p] = coeffs[zz];
                    }
                    for y in 0..8 {
                        for x in 0..8 {
                            let mut s = 0f32;
                            for v in 0..8 {
                                for u in 0..8 {
                                    s += C_UV[u]
                                        * C_UV[v]
                                        * nat[v * 8 + u] as f32
                                        * COS[v * 8 + y]
                                        * COS[u * 8 + x];
                                }
                            }
                            planes[c][(by + y).min(dh - 1) * dw + (bx + x).min(dw - 1)] = s * 0.25;
                        }
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(dw * dh);
        for idx in 0..dw * dh {
            let (y, cb, cr) = (planes[0][idx] + 128.0, planes[1][idx], planes[2][idx]);
            let clamp = |v: f32| v.round().clamp(0.0, 255.0) as u8;
            out.push([
                clamp(y + 1.402 * cr),
                clamp(y - 0.344_136 * cb - 0.714_136 * cr),
                clamp(y + 1.772 * cb),
            ]);
        }
        (dw, dh, out)
    }

    fn detached_pixels(rows: &[[u8; 3]]) -> Vec<[u8; 3]> {
        rows.to_vec()
    }

    fn find_marker(jpeg: &[u8], marker: u8) -> usize {
        (0..jpeg.len() - 1)
            .find(|&i| jpeg[i] == 0xff && jpeg[i + 1] == marker)
            .unwrap_or_else(|| panic!("marker {marker:#x} not found"))
    }

    fn decode_symbol(bits: &[bool], pos: &mut usize, table: &HuffTable) -> u8 {
        for len in 1u8..=16 {
            let mut code = 0u16;
            for k in 0..len as usize {
                code = (code << 1) | bits[*pos + k] as u16;
            }
            for sym in 0u8..=255 {
                let (tc, tl) = table.code(sym);
                if tl == len && tc == code {
                    *pos += len as usize;
                    return sym;
                }
            }
        }
        panic!("bad huffman code");
    }

    fn read_value(bits: &[bool], pos: &mut usize, size: u32) -> i32 {
        if size == 0 {
            return 0;
        }
        let mut v = 0i32;
        for _ in 0..size {
            v = (v << 1) | bits[*pos] as i32;
            *pos += 1;
        }
        if v < (1 << (size - 1)) {
            v -= (1 << size) - 1;
        }
        v
    }

    #[test]
    fn dct_dc_matches_average() {
        // 常量块 v：DC = 8·v·C(0)²/4 = 8·v（v=64 → DC=512），AC 全 0。
        let block = [64f32; 64];
        let mut out = [0f32; 64];
        fdct(&block, &mut out);
        assert!((out[0] - 512.0).abs() < 0.01, "dc={}", out[0]);
        assert!(out[1].abs() < 0.01 && out[8].abs() < 0.01);
    }

    #[test]
    fn bitwriter_packs_bits() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0b11110000, 8);
        w.flush();
        assert_eq!(w.out[0], 0b1011_1110);
    }
}

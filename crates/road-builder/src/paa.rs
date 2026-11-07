//! PAA texture reader: picks one mipmap and decodes it to RGBA. Supports DXT1, DXT5,
//! ARGB8888, ARGB4444, ARGB1555 and AI88, with LZO-compressed DXT mipmaps.
use anyhow::{Result, bail, ensure};

pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes the largest mipmap whose sides are at most `max` pixels (or the smallest one).
pub fn decode(b: &[u8], max: u32) -> Result<Texture> {
    ensure!(b.len() > 8, "Plik PAA jest za krótki");
    let kind = u16::from_le_bytes([b[0], b[1]]);
    let mut p = 2;
    while b.get(p..p + 4) == Some(b"GGAT") {
        let len = u32::from_le_bytes(b.get(p + 8..p + 12).unwrap_or(&[0; 4]).try_into()?) as usize;
        p += 12 + len;
    }
    let palette = u16::from_le_bytes(b.get(p..p + 2).unwrap_or(&[0; 2]).try_into()?) as usize;
    p += 2 + palette * 3;
    let mut chosen = None;
    while p + 7 <= b.len() {
        let w = u16::from_le_bytes([b[p], b[p + 1]]);
        let h = u16::from_le_bytes([b[p + 2], b[p + 3]]);
        if w == 0 || h == 0 {
            break;
        }
        let size = u32::from_le_bytes([b[p + 4], b[p + 5], b[p + 6], 0]) as usize;
        let start = p + 7;
        ensure!(start + size <= b.len(), "Uszkodzony mipmap PAA");
        let lzo = w & 0x8000 != 0;
        let (w, h) = ((w & 0x7fff) as u32, h as u32);
        chosen = Some((w, h, lzo, &b[start..start + size]));
        if w.max(h) <= max {
            break;
        }
        p = start + size;
    }
    let Some((w, h, lzo, data)) = chosen else {
        bail!("Brak mipmap w PAA")
    };
    let pixels = w as usize * h as usize;
    let blocks = w.div_ceil(4) as usize * h.div_ceil(4) as usize;
    let expected = match kind {
        0xFF01 => blocks * 8,
        0xFF05 => blocks * 16,
        0x8888 => pixels * 4,
        0x4444 | 0x1555 | 0x8080 => pixels * 2,
        other => bail!("Nieobsługiwany format PAA {other:04X}"),
    };
    let data = if lzo {
        crate::odol::lzo1x_decompress(data, expected)?.0
    } else {
        // Uncompressed mipmaps only; LZSS-packed ARGB data is not supported.
        ensure!(data.len() == expected, "Nieobsługiwana kompresja PAA");
        data.to_vec()
    };
    ensure!(data.len() >= expected, "Uszkodzony mipmap PAA");
    let mut rgba = vec![0u8; pixels * 4];
    match kind {
        0xFF01 | 0xFF05 => dxt(&data, w, h, kind == 0xFF05, &mut rgba),
        _ => {
            for i in 0..pixels {
                let px = match kind {
                    0x8888 => [
                        data[i * 4 + 2],
                        data[i * 4 + 1],
                        data[i * 4],
                        data[i * 4 + 3],
                    ],
                    0x4444 => {
                        let v = u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]);
                        let c = |s: u16| (((v >> s) & 15) * 17) as u8;
                        [c(8), c(4), c(0), c(12)]
                    }
                    0x1555 => {
                        let v = u16::from_le_bytes([data[i * 2], data[i * 2 + 1]]);
                        let c = |s: u16| (((v >> s) & 31) * 255 / 31) as u8;
                        [c(10), c(5), c(0), if v & 0x8000 != 0 { 255 } else { 0 }]
                    }
                    _ => [data[i * 2], data[i * 2], data[i * 2], data[i * 2 + 1]],
                };
                rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
            }
        }
    }
    Ok(Texture {
        width: w,
        height: h,
        rgba,
    })
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = (c >> 11) & 31;
    let g = (c >> 5) & 63;
    let b = c & 31;
    [
        (r * 255 / 31) as u8,
        (g * 255 / 63) as u8,
        (b * 255 / 31) as u8,
    ]
}

fn dxt(data: &[u8], w: u32, h: u32, dxt5: bool, out: &mut [u8]) {
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let block = if dxt5 { 16 } else { 8 };
    for by in 0..bh {
        for bx in 0..bw {
            let Some(b) = data.get((by * bw + bx) * block..(by * bw + bx + 1) * block) else {
                return;
            };
            let (alpha, color) = if dxt5 {
                (Some(&b[..8]), &b[8..])
            } else {
                (None, b)
            };
            let c0 = u16::from_le_bytes([color[0], color[1]]);
            let c1 = u16::from_le_bytes([color[2], color[3]]);
            let (a, b2) = (rgb565(c0), rgb565(c1));
            let mix = |x: u8, y: u8, wx: u16, wy: u16| {
                ((x as u16 * wx + y as u16 * wy) / (wx + wy)) as u8
            };
            let four = c0 > c1 || dxt5;
            let palette: [[u8; 4]; 4] = [
                [a[0], a[1], a[2], 255],
                [b2[0], b2[1], b2[2], 255],
                if four {
                    [
                        mix(a[0], b2[0], 2, 1),
                        mix(a[1], b2[1], 2, 1),
                        mix(a[2], b2[2], 2, 1),
                        255,
                    ]
                } else {
                    [
                        mix(a[0], b2[0], 1, 1),
                        mix(a[1], b2[1], 1, 1),
                        mix(a[2], b2[2], 1, 1),
                        255,
                    ]
                },
                if four {
                    [
                        mix(a[0], b2[0], 1, 2),
                        mix(a[1], b2[1], 1, 2),
                        mix(a[2], b2[2], 1, 2),
                        255,
                    ]
                } else {
                    [0, 0, 0, 0]
                },
            ];
            let bits = u32::from_le_bytes([color[4], color[5], color[6], color[7]]);
            let alphas = alpha.map(|a| {
                let (a0, a1) = (a[0] as u16, a[1] as u16);
                let mut v = [0u8; 8];
                v[0] = a0 as u8;
                v[1] = a1 as u8;
                for i in 2..8u16 {
                    v[i as usize] = if a0 > a1 {
                        ((a0 * (8 - i) + a1 * (i - 1)) / 7) as u8
                    } else if i < 6 {
                        ((a0 * (6 - i) + a1 * (i - 1)) / 5) as u8
                    } else if i == 6 {
                        0
                    } else {
                        255
                    };
                }
                let idx = u64::from_le_bytes([a[2], a[3], a[4], a[5], a[6], a[7], 0, 0]);
                (v, idx)
            });
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x >= w as usize || y >= h as usize {
                        continue;
                    }
                    let k = py * 4 + px;
                    let mut c = palette[((bits >> (k * 2)) & 3) as usize];
                    if let Some((v, idx)) = alphas {
                        c[3] = v[((idx >> (k * 3)) & 7) as usize];
                    }
                    let o = (y * w as usize + x) * 4;
                    out[o..o + 4].copy_from_slice(&c);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_dxt1_block_and_picks_the_mip_that_fits() {
        // Two mipmaps: 8x8 and 4x4. The 4x4 block is pure red (c0 = c1 = 0xF800).
        let mut b = vec![0x01, 0xFF];
        b.extend([0, 0]); // no palette
        b.extend(8u16.to_le_bytes());
        b.extend(8u16.to_le_bytes());
        b.extend([32, 0, 0]);
        b.extend([0u8; 32]);
        b.extend(4u16.to_le_bytes());
        b.extend(4u16.to_le_bytes());
        b.extend([8, 0, 0]);
        b.extend([0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0]);
        b.extend([0, 0, 0, 0]);
        let t = decode(&b, 4).unwrap();
        assert_eq!((t.width, t.height), (4, 4));
        assert_eq!(&t.rgba[..4], &[255, 0, 0, 255]);
        let t = decode(&b, 64).unwrap();
        assert_eq!((t.width, t.height), (8, 8));
    }

    /// A real DayZ road texture when the P: drive is mounted.
    #[test]
    fn decodes_dayz_road_texture() {
        let Ok(dir) = std::fs::read_dir("P:/dz/structures/roads/data") else {
            return;
        };
        let Some(path) = dir
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.to_string_lossy().to_lowercase().ends_with("_co.paa"))
        else {
            return;
        };
        let t = decode(&std::fs::read(&path).unwrap(), 256).unwrap();
        assert!(t.width <= 256 && t.width >= 4, "{}", path.display());
        let mean: u64 = t
            .rgba
            .chunks(4)
            .map(|p| p[0] as u64 + p[1] as u64 + p[2] as u64)
            .sum::<u64>()
            / (t.rgba.len() as u64 / 4);
        assert!(mean > 20 && mean < 740, "{} mean {mean}", path.display());
    }
}

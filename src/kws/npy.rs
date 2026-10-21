//! `.npy` 与 `.npz`（`np.savez` 写的、不压缩或 deflate 的 zip）的最小读法：
//! 只认小端 `f4` / `f8`、C 序，读出来一律转成 f64。

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Array {
    pub shape: Vec<usize>,
    pub data: Vec<f64>,
}

impl Array {
    /// 二维数组的第 `r` 行。
    pub fn row(&self, r: usize) -> &[f64] {
        let cols = self.shape[1];
        &self.data[r * cols..(r + 1) * cols]
    }
}

pub fn parse(bytes: &[u8]) -> Result<Array> {
    ensure!(
        bytes.len() >= 10 && &bytes[..6] == b"\x93NUMPY",
        "not an .npy array"
    );
    let (header_len, start) = match bytes[6] {
        1 => (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10),
        2 | 3 => {
            ensure!(bytes.len() >= 12, "truncated .npy header");
            (
                u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes")) as usize,
                12,
            )
        }
        v => bail!(".npy version {v} is not supported"),
    };
    let header = std::str::from_utf8(
        bytes
            .get(start..start + header_len)
            .context("truncated .npy header")?,
    )?;
    let descr = field(header, "descr")?;
    let fortran = field(header, "fortran_order")?;
    ensure!(
        fortran.trim() == "False",
        "Fortran-order arrays are not supported"
    );
    let shape_text = field(header, "shape")?;
    let shape: Vec<usize> = shape_text
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().context("bad .npy shape"))
        .collect::<Result<_>>()?;
    let n: usize = shape.iter().product();
    let body = &bytes[start + header_len..];
    let data = match descr.trim_matches(|c| c == '\'' || c == '"') {
        "<f8" => {
            ensure!(body.len() >= n * 8, "truncated .npy data");
            body.as_chunks::<8>()
                .0
                .iter()
                .take(n)
                .map(|&c| f64::from_le_bytes(c))
                .collect()
        }
        "<f4" => {
            ensure!(body.len() >= n * 4, "truncated .npy data");
            body.as_chunks::<4>()
                .0
                .iter()
                .take(n)
                .map(|&c| f32::from_le_bytes(c) as f64)
                .collect()
        }
        other => bail!(".npy dtype {other} is not supported (only <f4, <f8)"),
    };
    Ok(Array { shape, data })
}

/// 头里形如 `'key': value,` 的一项；`shape` 的值里有逗号，按括号取。
fn field<'a>(header: &'a str, key: &str) -> Result<&'a str> {
    let at = header
        .find(&format!("'{key}'"))
        .with_context(|| format!(".npy header has no {key}"))?;
    let rest = header[at + key.len() + 2..]
        .trim_start()
        .strip_prefix(':')
        .context("bad .npy header")?
        .trim_start();
    let end = if rest.starts_with('(') {
        rest.find(')').map(|i| i + 1)
    } else {
        rest.find([',', '}'])
    }
    .context("bad .npy header")?;
    Ok(&rest[..end])
}

pub fn load(path: &Path) -> Result<Array> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    parse(&bytes).with_context(|| path.display().to_string())
}

pub fn read_npz<R: Read + Seek>(reader: R) -> Result<HashMap<String, Array>> {
    let mut zip = zip::ZipArchive::new(reader)?;
    let mut out = HashMap::new();
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let Some(name) = file.name()?.strip_suffix(".npy").map(str::to_owned) else {
            continue;
        };
        let mut bytes = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut bytes)?;
        out.insert(name.clone(), parse(&bytes).with_context(|| name)?);
    }
    Ok(out)
}

pub fn load_npz(path: &Path) -> Result<HashMap<String, Array>> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    read_npz(std::io::BufReader::new(file)).with_context(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn npy(descr: &str, shape: &str, body: &[u8]) -> Vec<u8> {
        let mut header =
            format!("{{'descr': '{descr}', 'fortran_order': False, 'shape': {shape}, }}");
        while (10 + header.len() + 1) % 64 != 0 {
            header.push(' ');
        }
        header.push('\n');
        let mut out = b"\x93NUMPY\x01\x00".to_vec();
        out.extend((header.len() as u16).to_le_bytes());
        out.extend(header.as_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn reads_f8_matrix_and_f4_vector() {
        let body: Vec<u8> = [1.0f64, 2.0, 3.0, 4.0, 5.0, 6.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let a = parse(&npy("<f8", "(2, 3)", &body)).unwrap();
        assert_eq!(a.shape, vec![2, 3]);
        assert_eq!(a.row(1), &[4.0, 5.0, 6.0]);

        let body: Vec<u8> = [0.5f32, -1.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let a = parse(&npy("<f4", "(2,)", &body)).unwrap();
        assert_eq!((a.shape, a.data), (vec![2], vec![0.5, -1.0]));
    }

    #[test]
    fn refuses_other_dtypes() {
        assert!(parse(&npy("<i8", "(1,)", &[0; 8])).is_err());
    }
}

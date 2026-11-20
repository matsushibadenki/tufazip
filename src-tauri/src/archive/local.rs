//! TufaZIP Local v1: little-endian entries followed by a whole-stream CRC32.
//! A zero encoded length marks a stored block. Each block also checks its decoded CRC32.
use super::*;
use crate::predictive;
use std::io::{BufReader, BufWriter};

pub(super) const MAGIC: [u8; 8] = *b"TUFAZIPL";
const VERSION: u8 = 1;
const BLOCK: usize = 64 * 1024;
const NAME_LIMIT: usize = 4096;

struct Checked<T> {
    inner: T,
    hash: crc32fast::Hasher,
}
impl<T> Checked<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            hash: crc32fast::Hasher::new(),
        }
    }
}
impl<T: Write> Write for Checked<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(bytes)?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
impl<T: Read> Read for Checked<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(bytes)?;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
}

fn bytes<const N: usize>(r: &mut impl Read) -> Result<[u8; N]> {
    let mut data = [0; N];
    r.read_exact(&mut data).context("damaged")?;
    Ok(data)
}

pub(super) fn write(path: &Path, entries: &[Entry], opt: &Options, report: &Report) -> Result<()> {
    let file = File::create_new(path)?;
    let mut out = Checked::new(BufWriter::new(file));
    out.write_all(&MAGIC)?;
    out.write_all(&[VERSION, opt.level])?;
    out.write_all(&(entries.len() as u32).to_le_bytes())?;
    let mut buf = vec![0; BLOCK];
    for (i, entry) in entries.iter().enumerate() {
        report(&entry.name, i, entries.len())?;
        ensure!(entry.name.len() <= NAME_LIMIT, "invalidName");
        out.write_all(&(entry.name.len() as u16).to_le_bytes())?;
        out.write_all(entry.name.as_bytes())?;
        out.write_all(&[u8::from(entry.directory)])?;
        out.write_all(&((entry.mode & 0o777) as u16).to_le_bytes())?;
        out.write_all(&entry.size.to_le_bytes())?;
        if entry.directory {
            continue;
        }
        let mut src = open_input(&entry.path)?;
        let mut left = entry.size;
        while left > 0 {
            report(&entry.name, i, entries.len())?;
            let n = left.min(BLOCK as u64) as usize;
            let block = &mut buf[..n];
            src.read_exact(block)?;
            let encoded = if opt.level > 0 {
                predictive::encode(block, opt.level)
            } else {
                Vec::new()
            };
            let compressed = !encoded.is_empty() && encoded.len() < n;
            let length = if compressed { encoded.len() as u32 } else { 0 };
            out.write_all(&length.to_le_bytes())?;
            out.write_all(&crc32fast::hash(block).to_le_bytes())?;
            out.write_all(if compressed { &encoded } else { block })?;
            left -= n as u64;
        }
        ensure!(src.read(&mut buf[..1])? == 0, "sizeMismatch");
    }
    out.inner.write_all(&out.hash.finalize().to_le_bytes())?;
    out.inner.flush()?;
    out.inner.get_ref().sync_all()?;
    Ok(())
}

pub(super) fn read(path: &Path, dest: Option<&Path>, report: &Report) -> Result<()> {
    let mut src = Checked::new(BufReader::new(File::open(path)?));
    ensure!(bytes::<8>(&mut src)? == MAGIC, "format");
    let [version, level] = bytes(&mut src)?;
    ensure!(version == VERSION && level <= 3, "version");
    let count = u32::from_le_bytes(bytes(&mut src)?) as usize;
    ensure!(count <= MAX_ENTRIES, "limit");
    let mut total = 0u64;
    let mut names = HashSet::new();
    for i in 0..count {
        let len = u16::from_le_bytes(bytes(&mut src)?) as usize;
        ensure!(len > 0 && len <= NAME_LIMIT, "invalidName");
        let mut name = vec![0; len];
        src.read_exact(&mut name)?;
        let name = String::from_utf8(name).context("invalidName")?;
        let relative = safe_name(&name)?;
        ensure!(names.insert(relative.to_owned()), "duplicateName");
        report(&name, i, count)?;
        let [kind] = bytes(&mut src)?;
        let mode = u16::from_le_bytes(bytes(&mut src)?);
        let size = u64::from_le_bytes(bytes(&mut src)?);
        ensure!(kind <= 1 && (kind == 0 || size == 0), "damaged");
        total = total.checked_add(size).context("limit")?;
        ensure!(total <= MAX_BYTES, "limit");
        if kind == 1 {
            if let Some(dir) = dest {
                fs::create_dir_all(dir.join(relative))?;
            }
            continue;
        }
        let mut file = if let Some(dir) = dest {
            let path = dir.join(relative);
            fs::create_dir_all(path.parent().context("unsafePath")?)?;
            Some(File::create_new(path)?)
        } else {
            None
        };
        let mut left = size;
        while left > 0 {
            report(&name, i, count)?;
            let n = left.min(BLOCK as u64) as usize;
            let length = u32::from_le_bytes(bytes(&mut src)?) as usize;
            let crc = u32::from_le_bytes(bytes(&mut src)?);
            ensure!(length < n && (level != 0 || length == 0), "damaged");
            let mut block = vec![0; if length == 0 { n } else { length }];
            src.read_exact(&mut block).context("damaged")?;
            if length != 0 {
                block = predictive::decode(&block, n, level)?;
            }
            ensure!(crc32fast::hash(&block) == crc, "damaged");
            if let Some(f) = &mut file {
                f.write_all(&block)?;
            }
            left -= n as u64;
        }
        if let Some(f) = file {
            f.set_permissions(fs::Permissions::from_mode(0o600 | u32::from(mode & 0o155)))?;
        }
    }
    let sum = src.hash.finalize();
    ensure!(
        u32::from_le_bytes(bytes(&mut src.inner)?) == sum && src.inner.read(&mut [0])? == 0,
        "damaged"
    );
    Ok(())
}

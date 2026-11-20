//! Archive I/O operates entirely in a private staging directory on the destination volume.
mod local;
use crate::Options;
use anyhow::{Context, Result, bail, ensure};
use rustix::fs::{CWD, RenameFlags, renameat_with};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};
use walkdir::{DirEntry, WalkDir};
use zip::{AesMode, CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

const MAX_ENTRIES: usize = 100_000;
const MAX_BYTES: u64 = 128 * 1024 * 1024 * 1024;
const MAX_PARTS: usize = 10_000;
pub type Report<'a> = dyn Fn(&str, usize, usize) -> Result<()> + 'a;

fn label(p: &Path) -> Result<&str> {
    p.file_name()
        .and_then(|s| s.to_str())
        .context("invalidName")
}

fn safe_name(name: &str) -> Result<&Path> {
    let path = Path::new(name);
    ensure!(
        !name.is_empty()
            && !name.contains(['\\', '\0', ':'])
            && path.components().all(|c| matches!(c, Component::Normal(_)))
            && !name.split('/').any(|c| c == ".."),
        "unsafePath"
    );
    Ok(path)
}

fn mac_item(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if matches!(
        name.as_ref(),
        ".DS_Store" | "__MACOSX" | ".Trashes" | ".Spotlight-V100" | ".fseventsd"
    ) {
        return true;
    }
    if name.starts_with("._") && entry.file_type().is_file() {
        let mut magic = [0; 4];
        return File::open(entry.path())
            .and_then(|mut f| f.read_exact(&mut magic))
            .is_ok()
            && magic == [0, 5, 22, 7];
    }
    false
}

struct Entry {
    path: PathBuf,
    name: String,
    directory: bool,
    size: u64,
    mode: u32,
}

fn collect(paths: &[PathBuf], clean: bool, report: &Report) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut names = HashSet::new();
    let mut total = 0u64;
    for root in paths {
        let parent = root.parent().context("invalidName")?;
        for item in WalkDir::new(root)
            .follow_root_links(false)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| !clean || !mac_item(e))
        {
            let item = item?;
            let path = item.path();
            let name = path
                .strip_prefix(parent)?
                .to_str()
                .context("invalidName")?
                .to_owned();
            safe_name(&name)?;
            report(&name, 0, 0)?;
            ensure!(names.insert(name.clone()), "duplicateName");
            let meta = item.metadata()?;
            ensure!(meta.is_file() || meta.is_dir(), "unsupportedItem: {name}");
            let size = if meta.is_file() { meta.len() } else { 0 };
            total = total.checked_add(size).context("limit")?;
            ensure!(entries.len() < MAX_ENTRIES && total <= MAX_BYTES, "limit");
            entries.push(Entry {
                path: path.to_owned(),
                name,
                directory: meta.is_dir(),
                size,
                mode: meta.permissions().mode(),
            });
        }
    }
    ensure!(!entries.is_empty(), "empty");
    Ok(entries)
}

// Read to EOF to trigger the ZIP reader's CRC / AES checks, with a bound on actual output.
fn transfer(
    mut src: impl Read,
    mut dst: impl Write,
    size: u64,
    mut tick: impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut buf = [0u8; 65536];
    let mut copied = 0;
    loop {
        tick()?;
        let count = src.read(&mut buf)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        ensure!(copied <= size, "sizeMismatch");
        dst.write_all(&buf[..count])?;
    }
    ensure!(copied == size, "sizeMismatch");
    Ok(())
}

fn open_input(path: &Path) -> io::Result<File> {
    File::options()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
}

fn write_zip(path: &Path, entries: &[Entry], opt: &Options, report: &Report) -> Result<()> {
    let mut zip = ZipWriter::new(File::create_new(path)?);
    let method = if opt.level == 0 {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    };
    let level = [None, Some(1), Some(6), Some(9)][opt.level as usize];
    for (i, e) in entries.iter().enumerate() {
        report(&e.name, i, entries.len())?;
        let options = SimpleFileOptions::default().unix_permissions(e.mode & 0o777);
        if e.directory {
            zip.add_directory(&e.name, options)?;
        } else {
            let mut options = options
                .compression_method(method)
                .compression_level(level)
                .large_file(e.size >= 0xffff0000);
            if !opt.password.is_empty() {
                options = options.with_aes_encryption(AesMode::Aes256, &opt.password);
            }
            zip.start_file(&e.name, options)?;
            let file = open_input(&e.path)?;
            transfer(file, &mut zip, e.size, || report(&e.name, i, entries.len()))?;
        }
    }
    zip.finish()?.sync_all()?;
    Ok(())
}

fn read_zip(path: &Path, dest: Option<&Path>, password: &str, report: &Report) -> Result<()> {
    let mut zip = ZipArchive::new(File::open(path)?)?;
    let count = zip.len();
    ensure!(count <= MAX_ENTRIES, "limit");
    let mut total = 0u64;
    for i in 0..count {
        let mut entry = zip.by_index_decrypt(i, password.as_bytes())?;
        let name = entry.name()?.into_owned();
        let relative = safe_name(&name)?;
        ensure!(
            entry.enclosed_name().is_some() && !entry.is_symlink(),
            "unsafePath"
        );
        report(&name, i, count)?;
        let size = entry.size();
        total = total.checked_add(size).context("limit")?;
        ensure!(total <= MAX_BYTES, "limit");
        if entry.is_dir() {
            ensure!(size == 0, "sizeMismatch");
            if let Some(dir) = dest {
                fs::create_dir_all(dir.join(relative))?;
            }
            continue;
        }
        if let Some(dir) = dest {
            let out = dir.join(relative);
            fs::create_dir_all(out.parent().context("unsafePath")?)?;
            let mut file = File::create_new(&out)?;
            transfer(&mut entry, &mut file, size, || report(&name, i, count))?;
            if let Some(mode) = entry.unix_mode() {
                // Preserve executable files without restoring special bits or making extraction unwritable.
                file.set_permissions(fs::Permissions::from_mode(0o600 | (mode & 0o155)))?;
            }
        } else {
            transfer(&mut entry, io::sink(), size, || report(&name, i, count))?;
        }
    }
    Ok(())
}

fn split(path: &Path, dest: &Path, bytes: u64, report: &Report) -> Result<()> {
    fs::create_dir(dest)?;
    let mut src = File::open(path)?;
    let mut left = src.metadata()?.len();
    let count = left.div_ceil(bytes) as usize;
    ensure!(count <= MAX_PARTS, "limit");
    let base = label(path)?;
    for i in 1..=count {
        let size = left.min(bytes);
        let name = format!("{base}.{i:03}");
        transfer(
            (&mut src).take(size),
            File::create_new(dest.join(&name))?,
            size,
            || report(&name, i - 1, count),
        )?;
        left -= size;
    }
    Ok(())
}

fn join_parts(first: &Path, temp: &Path, report: &Report) -> Result<PathBuf> {
    let base = first.with_extension("");
    let name = label(&base)?;
    let dest = temp.join("joined");
    let mut output = File::create_new(&dest)?;
    let mut total = 0u64;
    for i in 1..=MAX_PARTS + 1 {
        let part = base.with_file_name(format!("{name}.{i:03}"));
        match fs::symlink_metadata(&part) {
            Ok(meta) => {
                ensure!(i <= MAX_PARTS && meta.is_file(), "badPart");
                total = total.checked_add(meta.len()).context("limit")?;
                ensure!(total <= MAX_BYTES + (1 << 30), "limit");
                transfer(File::open(&part)?, &mut output, meta.len(), || {
                    report(label(&part)?, 0, 0)
                })?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound && i > 1 => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(dest)
}

fn publish(staged: &Path, dir: &Path, name: &str, is_dir: bool) -> Result<PathBuf> {
    let path = Path::new(name);
    safe_name(name)?;
    ensure!(path.components().count() == 1, "invalidName");
    for number in 1..=9999 {
        let dest = if number == 1 {
            dir.join(name)
        } else if is_dir {
            dir.join(format!("{name} ({number})"))
        } else {
            let stem = path.file_stem().unwrap().to_string_lossy();
            let ext = path.extension().unwrap_or_default().to_string_lossy();
            dir.join(format!("{stem} ({number}).{ext}"))
        };
        match renameat_with(CWD, staged, CWD, &dest, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(dest),
            Err(rustix::io::Errno::EXIST | rustix::io::Errno::NOTEMPTY) => continue,
            Err(err) => return Err(err.into()),
        }
    }
    bail!("duplicateName")
}

pub fn run(inputs: &[PathBuf], dest: &Path, opt: &Options, report: &Report) -> Result<PathBuf> {
    ensure!(!inputs.is_empty(), "empty");
    ensure!(
        opt.level <= 3 && matches!(opt.split_mb, 0 | 5 | 25 | 100 | 1024),
        "settings"
    );
    let dest = dest.canonicalize()?;
    for input in inputs {
        ensure!(
            !dest.starts_with(input.canonicalize()?) || !input.is_dir(),
            "insideInput"
        );
    }
    let work = tempfile::tempdir_in(&dest)?;
    if opt.mode == "extract" {
        ensure!(inputs.len() == 1, "oneArchive");
        let input = &inputs[0];
        ensure!(fs::symlink_metadata(input)?.is_file(), "oneArchive");
        let joined = input.extension().is_some_and(|e| e == "001");
        let source = if joined {
            join_parts(input, work.path(), report)?
        } else {
            input.clone()
        };
        let archive_name = if joined {
            input.with_extension("")
        } else {
            input.clone()
        };
        let name = archive_name
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalidName")?;
        let folder = work.path().join("contents");
        fs::create_dir(&folder)?;
        let mut magic = [0; 8];
        if File::open(&source)?.read_exact(&mut magic).is_ok() && magic == local::MAGIC {
            local::read(&source, Some(&folder), report)?;
        } else {
            read_zip(&source, Some(&folder), &opt.password, report)?;
        }
        report(name, 1, 1)?;
        return publish(&folder, &dest, name, true);
    }
    ensure!(
        opt.mode == "compress" && matches!(opt.format.as_str(), "zip" | "local"),
        "format"
    );
    ensure!(
        opt.format != "local" || (opt.password.is_empty() && opt.confirm.is_empty()),
        "zipPasswordOnly"
    );
    ensure!(opt.password == opt.confirm, "passwordMismatch");
    let groups = work.path().join("output");
    fs::create_dir(&groups)?;
    let per_item = opt.separate && inputs.len() > 1;
    let batches: Vec<&[PathBuf]> = if per_item {
        inputs.chunks(1).collect()
    } else {
        vec![inputs]
    };
    let mut result = None;
    for batch in batches {
        let base = if batch.len() == 1 {
            label(&batch[0])?
        } else {
            "Archive"
        };
        let extension = if opt.format == "zip" {
            "zip"
        } else {
            "tzlocal"
        };
        let name = format!("{base}.{extension}");
        let stage = tempfile::tempdir_in(work.path())?;
        let archive = stage.path().join(&name);
        let entries = collect(batch, opt.clean, report)?;
        if opt.format == "zip" {
            write_zip(&archive, &entries, opt, report)?;
            if opt.verify {
                read_zip(&archive, None, &opt.password, report)?;
            }
        } else {
            local::write(&archive, &entries, opt, report)?;
            if opt.verify {
                local::read(&archive, None, report)?;
            }
        }
        let (output, filename, is_dir) = if opt.split_mb != 0 {
            let parts = stage.path().join("volumes");
            split(
                &archive,
                &parts,
                u64::from(opt.split_mb) * 1_000_000,
                report,
            )?;
            (parts, format!("{name}.parts"), true)
        } else {
            (archive, name, false)
        };
        report(&filename, 1, 1)?;
        result = Some(publish(
            &output,
            if per_item { &groups } else { &dest },
            &filename,
            is_dir,
        )?);
    }
    if per_item {
        publish(&groups, &dest, "Archives", true)
    } else {
        result.context("empty")
    }
}

#[cfg(test)]
mod tests;

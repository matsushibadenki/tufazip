use super::*;

fn silent(_: &str, _: usize, _: usize) -> Result<()> {
    Ok(())
}
fn extract(path: &Path, dest: &Path, password: &str) -> Result<PathBuf> {
    let opt = Options {
        mode: "extract".into(),
        password: password.into(),
        ..Default::default()
    };
    run(&[path.to_owned()], dest, &opt, &silent)
}

#[test]
fn mixed_directory_roundtrips_at_every_level() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().join("資料");
    fs::create_dir_all(root.join("empty"))?;
    let text = "同じ内容\n".repeat(14000).into_bytes();
    fs::write(root.join("日本語.txt"), &text)?;
    let mut seed = 3u32;
    let binary: Vec<_> = (0..70351)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    fs::write(root.join("bytes"), &binary)?;
    fs::write(root.join(".env"), b"")?;
    fs::write(root.join(".DS_Store"), b"ignored")?;
    fs::write(root.join("._real"), b"ordinary file")?;
    fs::write(root.join("._meta"), [0, 5, 22, 7])?;
    for format in ["zip", "local"] {
        for level in 0..=3 {
            let opt = Options {
                format: format.into(),
                level,
                verify: true,
                ..Default::default()
            };
            let archive = run(std::slice::from_ref(&root), tmp.path(), &opt, &silent)?;
            if format == "local" && level > 0 {
                assert!(archive.metadata()?.len() < (text.len() + binary.len()) as u64);
            }
            let output = extract(&archive, tmp.path(), "")?.join("資料");
            assert_eq!(fs::read(output.join("日本語.txt"))?, text);
            assert_eq!(fs::read(output.join("bytes"))?, binary);
            assert!(output.join("empty").is_dir() && output.join(".env").is_file());
            assert!(output.join("._real").exists() && !output.join("._meta").exists());
            assert!(!output.join(".DS_Store").exists());
        }
    }
    Ok(())
}

#[test]
fn aes_password_and_standard_zip_interop() -> Result<()> {
    use std::process::Command;
    let tmp = tempfile::tempdir()?;
    let input = tmp.path().join("sample.txt");
    fs::write(&input, b"sample file")?;
    let opts = Options {
        password: "秘密".into(),
        confirm: "秘密".into(),
        verify: true,
        ..Default::default()
    };
    let archive = run(std::slice::from_ref(&input), tmp.path(), &opts, &silent)?;
    assert!(extract(&archive, tmp.path(), "wrong").is_err());
    assert_eq!(
        fs::read(extract(&archive, tmp.path(), "秘密")?.join("sample.txt"))?,
        b"sample file"
    );
    let plain = run(&[input], tmp.path(), &Options::default(), &silent)?;
    let external = Command::new("/usr/bin/unzip")
        .arg("-p")
        .arg(plain)
        .arg("sample.txt")
        .output()?;
    assert!(external.status.success());
    assert_eq!(external.stdout, b"sample file");
    assert!(
        Command::new("/usr/bin/zip")
            .current_dir(tmp.path())
            .args(["-q", "external.zip", "sample.txt"])
            .status()?
            .success()
    );
    assert_eq!(
        fs::read(extract(&tmp.path().join("external.zip"), tmp.path(), "")?.join("sample.txt"))?,
        b"sample file"
    );
    Ok(())
}

#[test]
fn split_volumes_and_truncation() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let input = tmp.path().join("large");
    let content = vec![42; 5_030_000];
    fs::write(&input, &content)?;
    for format in ["local", "zip"] {
        let opt = Options {
            format: format.into(),
            level: 0,
            split_mb: 5,
            ..Default::default()
        };
        let parts = run(std::slice::from_ref(&input), tmp.path(), &opt, &silent)?;
        let ext = if format == "zip" { "zip" } else { "tzlocal" };
        let first = parts.join(format!("large.{ext}.001"));
        assert_eq!(
            fs::read(extract(&first, tmp.path(), "")?.join("large"))?,
            content
        );
        fs::remove_file(parts.join(format!("large.{ext}.002")))?;
        assert!(extract(&first, tmp.path(), "").is_err());
    }
    Ok(())
}

#[test]
fn local_damage_and_cancellation_publish_nothing() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let dest = tempfile::tempdir()?;
    let input = tmp.path().join("file");
    fs::write(&input, b"compress me".repeat(9000))?;
    let opt = Options {
        format: "local".into(),
        ..Default::default()
    };
    let archive = run(std::slice::from_ref(&input), tmp.path(), &opt, &silent)?;
    let mut data = fs::read(&archive)?;
    let midpoint = data.len() / 2;
    data[midpoint] ^= 32;
    fs::write(&archive, data)?;
    assert!(extract(&archive, dest.path(), "").is_err());
    let n = std::cell::Cell::new(0);
    let result = run(&[input], dest.path(), &opt, &|_, _, _| {
        n.set(n.get() + 1);
        ensure!(n.get() < 4, "cancelled");
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(dest.path().read_dir()?.count(), 0);
    Ok(())
}

#[test]
fn separate_archives_and_retained_sidecars() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let inputs = [tmp.path().join("one"), tmp.path().join("two")];
    for dir in &inputs {
        fs::create_dir(dir)?;
        fs::write(dir.join(".DS_Store"), b"test")?;
    }
    for format in ["zip", "local"] {
        let opt = Options {
            format: format.into(),
            separate: true,
            clean: false,
            ..Default::default()
        };
        let dir = run(&inputs, tmp.path(), &opt, &silent)?;
        assert_eq!(dir.read_dir()?.count(), 2);
        for item in dir.read_dir()? {
            let archive = item?.path();
            let stem = archive.file_stem().unwrap();
            assert_eq!(
                fs::read(
                    extract(&archive, tmp.path(), "")?
                        .join(stem)
                        .join(".DS_Store")
                )?,
                b"test"
            );
        }
    }
    Ok(())
}

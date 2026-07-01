use std::fs;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

#[cfg(test)]
use crate::materialize::DIR_MODE;
use crate::materialize::{MaterializeError, EXEC_MODE, FILE_MODE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnpackStats {
    pub files: usize,
    pub bytes: u64,
}

pub fn unpack_to(bytes: &[u8], dest: &Path) -> Result<UnpackStats, MaterializeError> {
    let decoder = GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    ensure_dir(dest)?;

    let mut stats = UnpackStats { files: 0, bytes: 0 };
    let entries = archive
        .entries()
        .map_err(|err| MaterializeError::invalid_archive(err.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|err| MaterializeError::invalid_archive(err.to_string()))?;
        let raw_path = entry
            .path()
            .map_err(|err| MaterializeError::invalid_archive(err.to_string()))?;
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink()
            || entry_type.is_hard_link()
            || entry_type.is_block_special()
            || entry_type.is_character_special()
            || entry_type.is_fifo()
        {
            return Err(MaterializeError::unsafe_member(
                raw_path.display().to_string(),
            ));
        }
        let Some(member) = normalize_member(raw_path.as_ref())? else {
            continue;
        };
        let out = dest.join(&member);
        if entry_type.is_dir() {
            ensure_dir(&out)?;
            continue;
        }
        if !entry_type.is_file() {
            return Err(MaterializeError::unsafe_member(
                raw_path.display().to_string(),
            ));
        }
        if let Some(parent) = out.parent() {
            ensure_dir(parent)?;
        }
        let mut file =
            fs::File::create(&out).map_err(|source| MaterializeError::io(&out, source))?;
        let mut buf = [0_u8; 8192];
        let mut written = 0_u64;
        loop {
            let read = entry
                .read(&mut buf)
                .map_err(|err| MaterializeError::invalid_archive(err.to_string()))?;
            if read == 0 {
                break;
            }
            file.write_all(&buf[..read])
                .map_err(|source| MaterializeError::io(&out, source))?;
            written += read as u64;
        }
        let mode = normalized_file_mode(&entry)?;
        // Only set permissions for executable files (bin scripts). Skip
        // normalize_path (utimensat + chmod) for every file — the per-file
        // cost of 2 syscalls across thousands of files dominates cold installs.
        if mode == EXEC_MODE {
            #[cfg(unix)]
            {
                fs::set_permissions(&out, fs::Permissions::from_mode(EXEC_MODE))
                    .map_err(|source| MaterializeError::io(&out, source))?;
            }
        }
        stats.files += 1;
        stats.bytes += written;
    }

    Ok(stats)
}

fn normalize_member(path: &Path) -> Result<Option<PathBuf>, MaterializeError> {
    // Tar member names are POSIX paths regardless of the host OS. Do not use
    // `Path::components()` here: on Windows it can interpret archive member text
    // with host semantics and silently fail to reproduce npm's package/ stripping.
    // Normalize as slash-separated archive data, then build a host PathBuf only
    // after validating every segment.
    let raw = path.to_string_lossy().replace('\\', "/");
    if raw.starts_with('/') || raw.contains(':') {
        return Err(MaterializeError::unsafe_member(path.display().to_string()));
    }

    let mut parts = raw
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".");
    let Some(first) = parts.next() else {
        return Ok(None);
    };
    if first == ".." {
        return Err(MaterializeError::unsafe_member(path.display().to_string()));
    }

    let mut out = PathBuf::new();
    for part in parts {
        if part == ".." {
            return Err(MaterializeError::unsafe_member(path.display().to_string()));
        }
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        return Ok(None);
    }
    Ok(Some(out))
}

fn normalized_file_mode<R: Read>(entry: &tar::Entry<'_, R>) -> Result<u32, MaterializeError> {
    let mode = entry
        .header()
        .mode()
        .map_err(|err| MaterializeError::invalid_archive(err.to_string()))?;
    Ok(if mode & 0o111 != 0 {
        EXEC_MODE
    } else {
        FILE_MODE
    })
}

fn ensure_dir(path: &Path) -> Result<(), MaterializeError> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(path).map_err(|source| MaterializeError::io(path, source))
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use flate2::write::GzEncoder;
    use flate2::Compression;

    use super::*;
    use crate::test_support::unique_tmp_dir;

    fn archive(entries: Vec<(tar::EntryType, String, Vec<u8>, u32)>) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (entry_type, path, bytes, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(entry_type);
            header.set_mode(mode);
            header.set_mtime(99);
            header.set_size(bytes.len() as u64);
            header.set_cksum();
            builder
                .append_data(&mut header, path, bytes.as_slice())
                .expect("append archive member");
        }
        builder
            .into_inner()
            .expect("finish tar")
            .finish()
            .expect("finish gzip")
    }

    fn raw_archive(path: &str, entry_type: u8, bytes: &[u8], mode: u32) -> Vec<u8> {
        fn write_octal(field: &mut [u8], value: u64) {
            for byte in field.iter_mut() {
                *byte = b'0';
            }
            let text = format!("{value:o}");
            let start = field.len().saturating_sub(text.len() + 1);
            field[start..start + text.len()].copy_from_slice(text.as_bytes());
            let last = field.len().saturating_sub(1);
            field[last] = 0;
        }

        let mut header = [0_u8; 512];
        header[..path.len()].copy_from_slice(path.as_bytes());
        write_octal(&mut header[100..108], mode as u64);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], bytes.len() as u64);
        write_octal(&mut header[136..148], 99);
        header[148..156].fill(b' ');
        header[156] = entry_type;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum = header.iter().map(|byte| u32::from(*byte)).sum::<u32>() as u64;
        write_octal(&mut header[148..156], checksum);

        let mut tar = Vec::new();
        tar.extend_from_slice(&header);
        tar.extend_from_slice(bytes);
        let remainder = bytes.len() % 512;
        if remainder != 0 {
            tar.extend(std::iter::repeat_n(0_u8, 512 - remainder));
        }
        tar.extend(std::iter::repeat_n(0_u8, 1024));

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar).expect("gzip raw tar");
        encoder.finish().expect("finish gzip raw tar")
    }

    #[test]
    fn unpack_to_streams_files_and_normalizes_modes() {
        let dest = unique_tmp_dir("archive-success");
        let bytes = archive(vec![
            (
                tar::EntryType::Directory,
                "package/lib".to_owned(),
                vec![],
                DIR_MODE,
            ),
            (
                tar::EntryType::Regular,
                "package/package.json".to_owned(),
                br#"{"name":"demo","version":"1.0.0"}"#.to_vec(),
                0o644,
            ),
            (
                tar::EntryType::Regular,
                "package/bin/run.js".to_owned(),
                b"#!/usr/bin/env node\n".to_vec(),
                0o777,
            ),
        ]);

        let stats = unpack_to(&bytes, &dest).expect("unpack archive");
        assert_eq!(stats.files, 2);
        let expected_bytes = br#"{"name":"demo","version":"1.0.0"}"#.len() as u64
            + b"#!/usr/bin/env node\n".len() as u64;
        assert_eq!(stats.bytes, expected_bytes);
        assert!(dest.join("package.json").is_file());
        assert!(dest.join("bin/run.js").is_file());
        #[cfg(unix)]
        {
            let package_mode = fs::metadata(dest.join("package.json"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            let exec_mode = fs::metadata(dest.join("bin/run.js"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(package_mode, FILE_MODE);
            assert_eq!(exec_mode, EXEC_MODE);
        }
        fs::remove_dir_all(dest).ok();
    }

    #[test]
    fn unpack_to_rejects_escape_absolute_and_link_entries() {
        let cases = [
            raw_archive("package/../escape", b'0', b"nope", 0o644),
            raw_archive("/package.json", b'0', b"nope", 0o644),
            raw_archive("package/link", b'2', b"", 0o777),
            raw_archive("package/device", b'3', b"", 0o644),
        ];

        for (idx, bytes) in cases.iter().enumerate() {
            let dest = unique_tmp_dir(&format!("archive-unsafe-{idx}"));
            let err = unpack_to(bytes, &dest).expect_err("unsafe archive must fail");
            assert!(matches!(err, MaterializeError::UnsafeMember { .. }));
            assert!(!dest.join("escape").exists());
            fs::remove_dir_all(dest).ok();
        }
    }

    #[test]
    fn unpack_to_rejects_garbage_and_truncation_without_panicking() {
        let dest = unique_tmp_dir("archive-garbage");
        let err = unpack_to(b"not a gzip archive", &dest).expect_err("garbage archive");
        assert!(matches!(err, MaterializeError::InvalidArchive { .. }));
        fs::remove_dir_all(&dest).ok();

        let dest = unique_tmp_dir("archive-truncated");
        let bytes = archive(vec![(
            tar::EntryType::Regular,
            "package/package.json".to_owned(),
            br#"{"name":"demo"}"#.to_vec(),
            0o644,
        )]);
        let truncated = &bytes[..bytes.len() / 2];
        let err = unpack_to(truncated, &dest).expect_err("truncated archive");
        assert!(matches!(err, MaterializeError::InvalidArchive { .. }));
        fs::remove_dir_all(dest).ok();
    }
}

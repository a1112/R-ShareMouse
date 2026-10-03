//! Strict preview profile values, independent of global environment in tests.
use std::{
    io,
    path::{Path, PathBuf},
};

pub fn port_value(value: Option<&str>, default: u16) -> io::Result<u16> {
    value.map_or(Ok(default), |value| {
        value
            .parse::<u16>()
            .ok()
            .filter(|v| *v != 0)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "IPC port must be an integer from 1 to 65535",
                )
            })
    })
}

pub fn profile_root(
    override_root: Option<&Path>,
    native_base: Option<&Path>,
) -> io::Result<PathBuf> {
    let root = override_root
        .map(Path::to_path_buf)
        .or_else(|| native_base.map(|p| p.join("rshare-rbox-preview")))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "Native user configuration directory unavailable",
            )
        })?;
    if !root.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "RSHARE_USER_ROOT must be absolute",
        ));
    }
    Ok(root)
}

pub fn require_owned_pid(expected: u32, actual: u32) -> io::Result<()> {
    if expected != 0 && expected == actual {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "IPC response belongs to another daemon process",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_preview_ports_never_fall_back_to_another_daemon() {
        assert_eq!(port_value(None, 27435).unwrap(), 27435);
        assert_eq!(port_value(Some("48123"), 27435).unwrap(), 48123);
        for value in ["0", "65536", "-1", "abc", ""] {
            assert!(port_value(Some(value), 27435).is_err());
        }
    }
    #[test]
    fn rejects_relative_profile_paths() {
        assert!(profile_root(Some(std::path::Path::new("relative")), None).is_err());
        let root = std::env::temp_dir().join("rshare-own-profile");
        assert_eq!(profile_root(Some(&root), None).unwrap(), root);
        assert!(profile_root(None, None).is_err());
    }
    #[test]
    fn a_foreign_status_pid_cannot_satisfy_owned_readiness() {
        assert!(require_owned_pid(21, 22).is_err());
        assert!(require_owned_pid(0, 0).is_err());
        assert!(require_owned_pid(21, 21).is_ok());
    }
}

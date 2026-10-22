use std::path::PathBuf;

fn uid() -> u32 {
    // SAFETY: getuid never fails.
    unsafe { libc::getuid() }
}

/// Unix socket the server listens on (`DTM_SOCKET` overrides).
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DTM_SOCKET") {
        return PathBuf::from(p);
    }
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("dtm").join("default.sock"),
        _ => PathBuf::from(format!("/tmp/dtm-{}", uid())).join("default.sock"),
    }
}

pub fn config_path() -> PathBuf {
    if let Some(p) = std::env::var_os("DTM_CONFIG") {
        return PathBuf::from(p);
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
    };
    base.join("dtm").join("config.toml")
}

pub fn log_path() -> PathBuf {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state"),
    };
    base.join("dtm").join("server.log")
}

/// Create the socket's parent directory, private to the user.
pub fn prepare_socket_dir(sock: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = sock.parent() {
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

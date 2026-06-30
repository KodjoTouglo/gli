//! Linux distribution detection for cross-distro behaviour.
//!
//! Reads `/etc/os-release` and classifies the host into a family so modules can
//! pick the right service names and package manager.

/// Broad distribution family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DistroFamily {
    Debian,
    Rhel,
    Arch,
    Suse,
    #[default]
    Unknown,
}

/// Detected host platform.
#[derive(Debug, Clone)]
pub struct Platform {
    pub family: DistroFamily,
    pub id: String,
}

impl Default for Platform {
    fn default() -> Self {
        Self {
            family: DistroFamily::Unknown,
            id: String::new(),
        }
    }
}

impl Platform {
    /// Detect from `/etc/os-release`; Unknown if it cannot be read.
    pub fn detect() -> Self {
        std::fs::read_to_string("/etc/os-release")
            .map(|c| Self::from_os_release(&c))
            .unwrap_or_default()
    }

    /// Build a platform for a known family (used by tests and defaults).
    pub fn of(family: DistroFamily) -> Self {
        Self {
            family,
            id: String::new(),
        }
    }

    /// Parse the contents of an os-release file.
    pub fn from_os_release(content: &str) -> Self {
        let mut id = String::new();
        let mut id_like = String::new();
        for line in content.lines() {
            if let Some(v) = line.strip_prefix("ID=") {
                id = unquote(v);
            } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
                id_like = unquote(v);
            }
        }
        Self {
            family: classify(&id, &id_like),
            id,
        }
    }

    /// systemd unit name for the SSH daemon.
    pub fn ssh_service(&self) -> &'static str {
        match self.family {
            DistroFamily::Debian => "ssh",
            _ => "sshd",
        }
    }

    /// System package manager command, if known.
    pub fn package_manager(&self) -> Option<&'static str> {
        match self.family {
            DistroFamily::Debian => Some("apt-get"),
            DistroFamily::Rhel => Some("dnf"),
            DistroFamily::Arch => Some("pacman"),
            DistroFamily::Suse => Some("zypper"),
            DistroFamily::Unknown => None,
        }
    }
}

fn classify(id: &str, id_like: &str) -> DistroFamily {
    let joined = format!("{id} {id_like}");
    let has = |k: &str| joined.split_whitespace().any(|t| t == k);
    if has("debian") || has("ubuntu") {
        DistroFamily::Debian
    } else if has("rhel") || has("fedora") || has("centos") || matches!(id, "rocky" | "almalinux") {
        DistroFamily::Rhel
    } else if has("arch") {
        DistroFamily::Arch
    } else if has("suse") || has("opensuse") {
        DistroFamily::Suse
    } else {
        DistroFamily::Unknown
    }
}

fn unquote(v: &str) -> String {
    v.trim().trim_matches('"').trim_matches('\'').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn family(os: &str) -> DistroFamily {
        Platform::from_os_release(os).family
    }

    #[test]
    fn detects_debian_and_ubuntu() {
        assert_eq!(family("ID=debian\n"), DistroFamily::Debian);
        assert_eq!(family("ID=ubuntu\nID_LIKE=debian\n"), DistroFamily::Debian);
    }

    #[test]
    fn detects_rhel_family() {
        assert_eq!(family("ID=fedora\n"), DistroFamily::Rhel);
        assert_eq!(
            family("ID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\n"),
            DistroFamily::Rhel
        );
        assert_eq!(family("ID=almalinux\n"), DistroFamily::Rhel);
    }

    #[test]
    fn ssh_service_per_family() {
        assert_eq!(Platform::of(DistroFamily::Debian).ssh_service(), "ssh");
        assert_eq!(Platform::of(DistroFamily::Rhel).ssh_service(), "sshd");
        assert_eq!(Platform::of(DistroFamily::Unknown).ssh_service(), "sshd");
    }

    #[test]
    fn package_manager_per_family() {
        assert_eq!(
            Platform::of(DistroFamily::Debian).package_manager(),
            Some("apt-get")
        );
        assert_eq!(
            Platform::of(DistroFamily::Rhel).package_manager(),
            Some("dnf")
        );
        assert_eq!(Platform::of(DistroFamily::Unknown).package_manager(), None);
    }
}

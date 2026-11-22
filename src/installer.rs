use crate::winget::Result;
use std::io;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{ProvisionProgress, provision, register_current_user};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provisioned {
    Installed,
    AlreadyCurrent,
}

pub struct Installation {
    pub provisioned: Provisioned,
    pub version: io::Result<String>,
    pub registration_error: Option<String>,
}

pub fn finish_installation(
    provisioned: Provisioned,
    register: impl FnOnce() -> Result<()>,
    verify: impl FnOnce() -> io::Result<String>,
) -> Installation {
    let registration_error = register().err().map(|error| error.to_string());
    Installation {
        provisioned,
        version: verify(),
        registration_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_missing_alias_does_not_erase_successful_provisioning() {
        let registered = Cell::new(false);
        let installation = finish_installation(
            Provisioned::Installed,
            || {
                registered.set(true);
                Ok(())
            },
            || {
                assert!(registered.get());
                Err(io::Error::other("The WinGet alias is unavailable."))
            },
        );

        assert_eq!(installation.provisioned, Provisioned::Installed);
        assert_eq!(
            installation.version.unwrap_err().to_string(),
            "The WinGet alias is unavailable."
        );
        assert!(installation.registration_error.is_none());
    }

    #[test]
    fn a_registration_error_can_leave_an_existing_alias_working() {
        let installation = finish_installation(
            Provisioned::AlreadyCurrent,
            || Err("Registration failed.".into()),
            || Ok("v1.26.0".into()),
        );

        assert_eq!(installation.provisioned, Provisioned::AlreadyCurrent);
        assert_eq!(installation.version.unwrap(), "v1.26.0");
        assert_eq!(
            installation.registration_error.as_deref(),
            Some("Registration failed.")
        );
    }
}

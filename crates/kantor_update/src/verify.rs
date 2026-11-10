//! Checks on a downloaded build before it replaces the running one, behind a
//! trait so tests (and Linux CI) can stand in for `codesign` and `spctl`.

use std::path::Path;
use std::process::Command;

use crate::Error;

/// Kantor's Apple Developer ID team.
pub const TEAM_ID: &str = "F6Q6UFUK69";
/// `CFBundleIdentifier` in scripts/macos-app.sh.
pub const BUNDLE_ID: &str = "com.dzulfikar.kantor";

pub trait Verifier: Send + Sync {
    /// macOS: `app` is signed by Kantor's team, intact, and notarised.
    fn check_app(&self, app: &Path) -> Result<(), Error>;

    /// Run `program args…` and return what it prints (stdout), to check the
    /// new build starts and reports the version the manifest promised.
    fn probe(&self, program: &Path, args: &[&str]) -> Result<String, Error>;
}

/// The real checks.
pub struct SystemVerifier;

fn run(cmd: &mut Command) -> Result<std::process::Output, Error> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    cmd.stdin(std::process::Stdio::null());
    // A file we just wrote can be briefly "busy" (ETXTBSY): another thread
    // forked while our write handle was open, and the child hasn't exec'd yet.
    let mut tries = 0;
    loop {
        match cmd.output() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 40 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            r => return r.map_err(|e| Error::Verify(format!("can't run {name}: {e}"))),
        }
    }
}

impl Verifier for SystemVerifier {
    fn check_app(&self, app: &Path) -> Result<(), Error> {
        // The designated requirement pins Apple's Developer ID chain, the
        // bundle id and the team; --deep --strict checks every nested piece.
        let requirement = format!(
            "=anchor apple generic and identifier \"{BUNDLE_ID}\" and \
             certificate leaf[subject.OU] = \"{TEAM_ID}\""
        );
        let out = run(Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict", "-R"])
            .arg(&requirement)
            .arg(app))?;
        if !out.status.success() {
            return Err(Error::Verify(format!(
                "codesign rejected the new Kantor.app: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        let out = run(Command::new("/usr/bin/codesign")
            .args(["-d", "--verbose=2"])
            .arg(app))?;
        // codesign -d writes its report to stderr.
        let info = String::from_utf8_lossy(&out.stderr);
        check_codesign_display(&info)?;
        let out = run(Command::new("/usr/sbin/spctl")
            .args(["--assess", "--type", "execute"])
            .arg(app))?;
        if !out.status.success() {
            return Err(Error::Verify(format!(
                "Gatekeeper rejected the new Kantor.app (not notarised?): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    fn probe(&self, program: &Path, args: &[&str]) -> Result<String, Error> {
        let out = run(Command::new(program).args(args))?;
        if !out.status.success() {
            return Err(Error::Verify(format!(
                "the new build didn't start ({})",
                out.status
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// The `codesign -d --verbose=2` report names Kantor's team, its bundle id,
/// and is not ad-hoc.
pub fn check_codesign_display(info: &str) -> Result<(), Error> {
    let field = |name: &str| {
        info.lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
            .map(str::trim)
    };
    if info.contains("Signature=adhoc") {
        return Err(Error::Verify("the new Kantor.app is ad-hoc signed".into()));
    }
    if field("TeamIdentifier") != Some(TEAM_ID) {
        return Err(Error::Verify(format!(
            "the new Kantor.app is signed by team {:?}, not {TEAM_ID}",
            field("TeamIdentifier").unwrap_or("none")
        )));
    }
    if field("Identifier") != Some(BUNDLE_ID) {
        return Err(Error::Verify(format!(
            "the new app is {:?}, not {BUNDLE_ID}",
            field("Identifier").unwrap_or("unknown")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codesign_report() {
        let good = "Executable=/x/Kantor.app/Contents/MacOS/kantor\n\
                    Identifier=com.dzulfikar.kantor\nFormat=app bundle with Mach-O universal\n\
                    Authority=Developer ID Application: Dzulfikar Ats Tsauri (F6Q6UFUK69)\n\
                    TeamIdentifier=F6Q6UFUK69\n";
        check_codesign_display(good).unwrap();
        let other_team = good.replace("TeamIdentifier=F6Q6UFUK69", "TeamIdentifier=ABCDE12345");
        assert!(check_codesign_display(&other_team).is_err());
        let adhoc = "Identifier=com.dzulfikar.kantor\nSignature=adhoc\nTeamIdentifier=not set\n";
        assert!(check_codesign_display(adhoc).is_err());
        let other_app = good.replace("Identifier=com.dzulfikar.kantor", "Identifier=com.evil.app");
        assert!(check_codesign_display(&other_app).is_err());
    }
}

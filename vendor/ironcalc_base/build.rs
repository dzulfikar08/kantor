// KANTOR PATCH: upstream asks `git describe` for the release (INFO("release")).
// Vendored inside the Kantor repo that would report Kantor's commit, so the
// value a crates.io build gets ("unknown": no git repository) is fixed here.
fn main() {
    println!("cargo:rustc-env=GIT_VERSION=unknown");
}

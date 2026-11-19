fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    if std::env::var("GITHUB_ACTIONS").as_deref() != Ok("true") {
        panic!("HostNR builds are restricted to GitHub Actions. Run Android CI, not a local build.");
    }
}

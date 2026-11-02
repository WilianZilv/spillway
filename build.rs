// The panel is embedded with include_dir!; make sure it exists and rebuild when it changes.
fn main() {
    let dist = std::path::Path::new("web/dist/index.html");
    if !dist.exists() {
        panic!("web/dist is missing: build the panel first (cd web && pnpm install && pnpm build)");
    }
    println!("cargo:rerun-if-changed=web/dist");
}

fn main() {
    // Exe icon for Explorer, Start menu search and Task Manager.
    embed_resource::compile("assets/wattray.rc", embed_resource::NONE).manifest_optional().unwrap();
}

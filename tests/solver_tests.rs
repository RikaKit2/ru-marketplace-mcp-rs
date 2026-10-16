use ru_marketplace_mcp::session::NativeCdpSolver;

#[test]
fn test_find_chromium_locates_binary_or_fallback() {
    let chromium_path = NativeCdpSolver::find_chromium();
    assert!(!chromium_path.as_os_str().is_empty());
    // On this NixOS machine, /run/current-system/sw/bin/chromium exists and should be found
    if std::path::Path::new("/run/current-system/sw/bin/chromium").exists() {
        assert_eq!(
            chromium_path,
            std::path::PathBuf::from("/run/current-system/sw/bin/chromium")
        );
    }
}

#[test]
fn test_ensure_xvfb_returns_display_99() {
    let display = NativeCdpSolver::ensure_xvfb();
    assert_eq!(display, ":99");
}

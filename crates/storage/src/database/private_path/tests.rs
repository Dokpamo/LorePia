use super::*;

#[test]
fn unchanged_permissions_do_no_writes_but_special_bits_and_widened_modes_are_repaired() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let child = root.join("child");
    let file = child.join("file");
    fs::create_dir(&child).unwrap();
    fs::write(&file, b"synthetic").unwrap();
    harden_owned_tree_permissions(root).unwrap();
    let mut calls = 0;
    harden_tree(root, &mut |path, directory| {
        calls += 1;
        harden_private_path(path, directory)
    })
    .unwrap();
    assert_eq!(calls, 0);
    fs::set_permissions(&child, fs::Permissions::from_mode(0o1700)).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    harden_tree(root, &mut |path, directory| {
        calls += 1;
        harden_private_path(path, directory)
    })
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(
        fs::metadata(&child).unwrap().permissions().mode() & 0o7777,
        0o700
    );
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    std::os::unix::fs::symlink(&file, root.join("link")).unwrap();
    assert!(harden_owned_tree_permissions(root).is_err());
}

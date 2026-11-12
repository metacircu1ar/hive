use super::*;
#[test]
fn captured_image_uses_the_admitted_inode_after_its_name_is_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let binary = temp.path().join("installer");
    fs::write(&binary, b"original image").unwrap();
    let image = Image::at(&binary).unwrap();
    fs::rename(&binary, temp.path().join("old")).unwrap();
    fs::write(&binary, b"replacement image").unwrap();
    let root = temp.path().join("generation");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    image.copy(&root).unwrap();
    assert_eq!(
        fs::read(root.join("bin/hive-install")).unwrap(),
        b"original image"
    );
    assert_eq!(fs::read(binary).unwrap(), b"replacement image");
}

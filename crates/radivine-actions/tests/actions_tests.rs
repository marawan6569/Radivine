//! Unit tests for `radivine-actions`.

use radivine_actions::builtin_registry;
use radivine_core::{
    ActionId, BasicContext, Context, InvocationMode, ItemPath, MenuLimits, SourceInfo,
};

#[test]
fn test_builtin_registry_builds_expected_3level_menu_and_paths() {
    let reg = builtin_registry();
    let ctx = Context::from_basic(BasicContext::from_uris(
        ["file:///home/user/report.txt"],
        InvocationMode::Invoke,
        SourceInfo::default(),
    ));

    let (tree, index) = reg
        .build_menu(&ctx, MenuLimits::default())
        .expect("default menu should build for a file context");

    let roots = tree.root.children().unwrap();
    assert_eq!(roots.len(), 4);
    assert_eq!(roots[0].id.as_ref(), "radivine.preview");
    assert_eq!(roots[1].id.as_ref(), "radivine.copy");
    assert_eq!(roots[2].id.as_ref(), "radivine.properties");
    assert_eq!(roots[3].id.as_ref(), "radivine.custom");

    // Verify exact ItemPaths required by AC-2.3 and AC-3.6
    assert_eq!(
        index.get(&ItemPath::from_slice(&[0, 0])),
        Some(&ActionId::from("radivine.preview.full"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[0, 1])),
        Some(&ActionId::from("radivine.preview.head"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[0, 2])),
        Some(&ActionId::from("radivine.preview.tail"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[1])),
        Some(&ActionId::from("radivine.copy"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[2])),
        Some(&ActionId::from("radivine.properties"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[3, 0])),
        Some(&ActionId::from("custom.terminal"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[3, 1, 0])),
        Some(&ActionId::from("custom.scripts.sha256"))
    );
    assert_eq!(
        index.get(&ItemPath::from_slice(&[3, 1, 1])),
        Some(&ActionId::from("custom.scripts.chmod"))
    );
}

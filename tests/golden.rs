//! ゴールデンテスト: `tests/golden/*.mal` を実行し、期待 stdout と一致するか検証する。
//!
//! 期待値は同名の `.expected` ファイルに書く。`error.mal` のみ exit code 1 を期待する。

use std::process::Command;

#[test]
fn golden() {
    let mut tested = 0;
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "mal"))
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();

        let out = Command::new(env!("CARGO_BIN_EXE_mal"))
            .arg(&path)
            .output()
            .expect("mal バイナリを実行できません");
        let stdout = String::from_utf8_lossy(&out.stdout);

        if name == "error" {
            // error.mal: exit code 1 で stdout が空であることを期待
            assert!(!out.status.success(), "{}: エラーを期待したのに成功した", name);
            assert_eq!(stdout, "", "{}: エラー時に stdout は空のはず", name);
        } else {
            let expected_path = path.with_extension("expected");
            let expected = std::fs::read_to_string(&expected_path)
                .unwrap_or_else(|_| panic!("期待値ファイルがありません: {:?}", expected_path));
            assert!(out.status.success(), "{}: 実行失敗: {}", name, String::from_utf8_lossy(&out.stderr));
            assert_eq!(stdout, expected, "{}: stdout 不一致", name);
        }
        tested += 1;
    }
    assert!(tested >= 1, "ゴールデンテストが 1 つも見つかりませんでした");
}

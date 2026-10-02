#!/usr/bin/env python3
"""Q03 AST grader contracts, using trusted synthetic Rust sources only."""
from __future__ import annotations

from pathlib import Path
import subprocess
import tempfile
import unittest
import json
import runpy
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
HELPER = '''fn rate_table(region: &str) -> Option<(u32, u32)> {
    match region { "local" => Some((5, 9)), "remote" => Some((12, 20)), _ => None }
}
'''
SHIPPING = '''pub fn shipping_rate(region: &str, expedited: bool) -> Option<u32> {
    rate_table(region).map(|(standard, fast)| if expedited { fast } else { standard })
}
'''


class Q03AstOracle(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.controller = runpy.run_path(str(ROOT / "evals/live/accept.py"),
                                       run_name="trusted_oracle_contract_test")
        cls.binary, cls.provenance = cls.controller["prepare_structure_checker"]()

    def grade(self, source):
        with tempfile.TemporaryDirectory(prefix="harness-structure-contract-") as directory:
            path = Path(directory) / "lib.rs"
            path.write_text(source)
            process = subprocess.run([str(self.binary), str(path)], capture_output=True,
                                     text=True, timeout=10)
            self.assertEqual(process.stderr, "")
            result = json.loads(process.stdout)
            return process.returncode, result

    def assert_pass(self, source):
        code, result = self.grade(source)
        self.assertEqual(code, 0, result)
        self.assertEqual(result["status"], "PASS")
        self.assertEqual(set(result["checks"]), {
            "private_rate_table", "single_local_literal", "single_remote_literal",
            "shipping_calls_table", "shipping_no_region_literals"})
        self.assertTrue(all(result["checks"].values()))

    def assert_fail(self, source, check):
        code, result = self.grade(source)
        self.assertEqual(code, 1, result)
        self.assertEqual(result["status"], "FAIL")
        self.assertFalse(result["checks"][check], result)

    def test_valid_helper_before_shipping(self):
        self.assert_pass(HELPER + SHIPPING)

    def test_valid_helper_after_shipping(self):
        self.assert_pass(SHIPPING + HELPER)

    def test_comments_docs_strings_and_raw_braces_do_not_change_function_boundaries(self):
        noise = '''// } pub fn shipping_rate(region: &str) { "local" "remote" }
/* fn rate_table(region: &str) { "local" } */
const NOISE: &str = r###"} fn shipping_rate { \"local\" \"remote\" #[cfg(test)]"###;
'''
        self.assert_pass(noise + '#[doc = "local"]\n' + SHIPPING
                         + '#[doc = "remote"]\n' + HELPER)

    def test_recursive_cfg_test_items_excluded_before_and_after_functions(self):
        tests = '''#[cfg( test )] mod before { const REGION: &str = "local"; }
mod nested { #[cfg(all(test, feature="some_feature"))]
    fn test_only() { let _ = "remote"; let _ = "local"; }
}
#[cfg(test)] fn test_only() { let _ = "remote"; }
'''
        self.assert_pass(tests + SHIPPING + HELPER + tests.replace("before", "after")
                         .replace("nested", "nested_after").replace("fn test_only()", "fn another_test_only()"))

    def test_cfg_test_impl_methods_are_excluded(self):
        self.assert_pass(HELPER + SHIPPING + '''struct Example;
impl Example { #[cfg(test)] fn sample() { let _ = "local"; let _ = "remote"; } }
''')

    def test_helper_missing(self):
        self.assert_fail(SHIPPING, "private_rate_table")

    def test_helper_public(self):
        self.assert_fail(HELPER.replace("fn rate_table", "pub fn rate_table") + SHIPPING,
                         "private_rate_table")

    def test_helper_signature_wrong(self):
        for changed in (HELPER.replace("region: &str", "region: String"),
                        HELPER.replace("Option<(u32, u32)>", "Option<(u64, u32)>"),
                        HELPER.replace("fn rate_table", "async fn rate_table")):
            with self.subTest(helper=changed):
                self.assert_fail(changed + SHIPPING, "private_rate_table")

    def test_mapping_must_be_in_private_helper(self):
        mapping_elsewhere = HELPER.replace("fn rate_table", "fn other_mapping")
        empty_helper = "fn rate_table(region: &str) -> Option<(u32,u32)> { None }\n"
        self.assert_fail(mapping_elsewhere + SHIPPING + empty_helper, "private_rate_table")

    def test_duplicate_production_region_literals(self):
        for region, check in [("local", "single_local_literal"),
                              ("remote", "single_remote_literal")]:
            with self.subTest(region=region):
                self.assert_fail(HELPER + SHIPPING + f'const DUPLICATE: &str = "{region}";\n', check)

    def test_region_literals_inside_shipping(self):
        for region in ["local", "remote"]:
            with self.subTest(region=region):
                self.assert_fail(HELPER + SHIPPING.replace("    rate_table", f'    let _ = "{region}";\n    rate_table'),
                                 "shipping_no_region_literals")

    def test_nondelegating_shipping_does_not_pass_because_later_helper_exists(self):
        self.assert_fail('''pub fn shipping_rate(region: &str, expedited: bool) -> Option<u32> {
    Some(if expedited { 9 } else { 5 })
}
''' + HELPER, "shipping_calls_table")

    def test_comment_or_string_call_is_not_ast_call(self):
        for statement in ['// rate_table(region)\n', 'let _ = "rate_table(region)";\n']:
            with self.subTest(statement=statement):
                self.assert_fail(HELPER + '''pub fn shipping_rate(region: &str, expedited: bool) -> Option<u32> {
''' + statement + "None\n}\n", "shipping_calls_table")

    def test_call_in_nested_function_does_not_count_as_shipping_delegation(self):
        shipping = '''pub fn shipping_rate(region: &str, expedited: bool) -> Option<u32> {
    fn uncalled(region: &str) { let _ = rate_table(region); }
    None
}
'''
        self.assert_fail(HELPER + shipping, "shipping_calls_table")

    def test_production_macro_region_literals_are_counted(self):
        self.assert_fail(HELPER + SHIPPING + 'fn noisy() { println!("local"); }\n',
                         "single_local_literal")

    def test_unknown_cfg_features_cannot_hide_duplicate_production_literals(self):
        self.assert_fail(HELPER + SHIPPING + '''#[cfg(any(test, feature="production_feature"))]
const DUPLICATE: &str = "local";
''', "single_local_literal")

    def test_raw_and_escaped_region_literals_are_actual_string_values(self):
        self.assert_pass(HELPER.replace('"local"', 'r#"local"#')
                         .replace('"remote"', '"\\x72emote"') + SHIPPING)

    def test_malformed_rust_is_error_with_bounded_source_free_message(self):
        code, result = self.grade('fn rate_table( CANARY_SOURCE_DO_NOT_ECHO')
        self.assertEqual(code, 2)
        self.assertEqual(result["status"], "ERROR")
        self.assertNotIn("CANARY", json.dumps(result))

    def test_oversized_source_is_error(self):
        code, result = self.grade("//" + "x" * (2 * 1024 * 1024))
        self.assertEqual(code, 2)
        self.assertEqual(result["status"], "ERROR")

    def test_controller_api_attaches_source_and_executable_provenance(self):
        with tempfile.TemporaryDirectory(prefix="harness-structure-api-") as directory:
            candidate = Path(directory)
            (candidate / "src").mkdir()
            (candidate / "src/lib.rs").write_text(SHIPPING + HELPER)
            result = self.controller["structural_check"](candidate)
        self.assertEqual(result["status"], "PASS", result)
        self.assertEqual(result["checker_source_sha256"],
                         self.controller["structure_checker_sources_hash"]())
        self.assertEqual(result["checker_executable_sha256"],
                         self.provenance["checker_executable_sha256"])
        self.assertEqual(set(result["checker_source_files_sha256"]),
                         {"Cargo.toml", "Cargo.lock", "src/main.rs"})
        self.assertTrue(result["checker_build"]["locked"])
        self.assertTrue(result["checker_build"]["offline"])

    def test_controller_syntax_error_fails_closed_without_regex_fallback(self):
        with tempfile.TemporaryDirectory(prefix="harness-structure-invalid-") as directory:
            candidate = Path(directory)
            (candidate / "src").mkdir()
            (candidate / "src/lib.rs").write_text(HELPER + SHIPPING + "INVALID_RUST(")
            result = self.controller["structural_check"](candidate)
        self.assertEqual(result["status"], "ERROR", result)
        self.assertIn("checker_source_sha256", result)

    def test_controller_missing_checker_sources_fails_closed(self):
        with tempfile.TemporaryDirectory(prefix="harness-missing-checker-") as directory:
            candidate = Path(directory)
            (candidate / "src").mkdir()
            (candidate / "src/lib.rs").write_text(HELPER + SHIPPING)
            globals_ = self.controller["structural_check"].__globals__
            with mock.patch.dict(globals_, {"STRUCTURE_CHECKER": candidate / "missing"}):
                result = self.controller["structural_check"](candidate)
        self.assertEqual(result["status"], "ERROR", result)
        self.assertEqual(result["checks"], {})

    def test_controller_unavailable_build_has_bounded_error_without_fallback(self):
        with tempfile.TemporaryDirectory(prefix="harness-unavailable-checker-") as directory:
            candidate = Path(directory)
            (candidate / "src").mkdir()
            (candidate / "src/lib.rs").write_text(HELPER + SHIPPING)
            globals_ = self.controller["structural_check"].__globals__
            def unavailable():
                raise RuntimeError("PRIVATE_BUILD_OUTPUT_MUST_NOT_BE_EXPOSED")
            with mock.patch.dict(globals_, {"prepare_structure_checker": unavailable}):
                result = self.controller["structural_check"](candidate)
        self.assertEqual(result["status"], "ERROR", result)
        self.assertNotIn("PRIVATE", json.dumps(result))
        self.assertIn("no fallback", result["error"])


if __name__ == "__main__":
    unittest.main()

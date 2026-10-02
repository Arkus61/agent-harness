#!/usr/bin/env python3
"""Assertions for the benchmark controller's honest freeze/acceptance boundary."""
import importlib.util
import json
from pathlib import Path
import tempfile
import types
import unittest

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('benchmark_suite', ROOT / 'suite.py') if (ROOT / 'suite.py').exists() else None
suite = types.SimpleNamespace()
if spec is not None:
    suite = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(suite)

class ControllerTests(unittest.TestCase):
    def api(self, name):
        function = getattr(suite, name, None)
        self.assertTrue(callable(function), f'missing controller capability: {name}')
        return function

    def test_thirty_distinct_cases_cover_each_catalogue_instance(self):
        cases = self.api('cases')()
        expected = {f'Q{family:02}-{suffix}' for family in range(1, 11) for suffix in ['D1','D2','H1']}
        self.assertEqual({case['id'] for case in cases}, expected)
        self.assertEqual(len(cases), 30)
        fingerprints = [json.dumps({key: c[key] for key in ('baseline','requirements','oracle')},sort_keys=True) for c in cases]
        self.assertEqual(len(set(fingerprints)), 30)

    def test_tasks_use_identical_bounded_budget_and_safe_profile(self):
        cases = self.api('cases')()
        specs = [self.api('task_spec')(c) for c in cases]
        self.assertEqual(len({json.dumps(s['budget'],sort_keys=True) for s in specs}),1)
        for spec in specs:
            self.assertEqual(spec['profile'],'isolated')
            self.assertEqual(spec['decision_mode'],'enforced')
            self.assertEqual(spec['provider']['model'],'gpt-6.1-sol')
            self.assertIn('Cargo.toml',spec['protected_paths'])
            self.assertEqual(spec['grants']['commands'],[])

    def test_only_h_configuration_available_without_real_ablation_flags(self):
        configs = self.api('configurations')()
        self.assertEqual(configs['H']['status'],'available')
        self.assertEqual(configs['B0']['status'],'unavailable')
        self.assertEqual(configs['B1']['status'],'unavailable')
        self.assertFalse(configs['B0']['fair_comparison_executed'])

    def test_materialized_seed_does_not_contain_external_oracle_or_control(self):
        case = self.api('cases')()[0]
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            self.api('materialize')(case,target)
            self.assertFalse((target/'oracle.rs').exists())
            self.assertFalse((target/'control').exists())
            self.assertTrue((target/'Cargo.lock').is_file())
            self.assertEqual((target/'src/lib.rs').read_text(),case['baseline']['src/lib.rs'])

    def test_high_context_seed_has_actual_ten_thousand_files(self):
        case = next(c for c in self.api('cases')() if c['id']=='Q08-D1')
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            self.api('materialize')(case,target)
            self.assertEqual(len(list((target/'catalog').rglob('*.txt'))),10000)
            self.assertIn('shipping surcharge rule',(target/'catalog/route-07319.txt').read_text())

    def test_receipt_cannot_be_reused_for_another_candidate(self):
        verify=self.api('verify_receipt')
        expected={'candidate_sha':'a'*40,'candidate_tree':'b'*40,'task_sha256':'c'*64,'oracle_sha256':'d'*64,'suite_sources_sha256':'e'*64}
        receipt={**expected,'status':'PASS'}
        self.assertTrue(verify(receipt,expected))
        for field in expected:
            mutated=dict(receipt);mutated[field]='f'*len(expected[field])
            self.assertFalse(verify(mutated,expected),field)
        self.assertFalse(verify({**receipt,'status':'ERROR'},expected))

    def test_freeze_detects_changed_sources(self):
        require=self.api('require_frozen')
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/'data';path.write_text('frozen')
            digest=self.api('file_hash')(path)
            require(path,digest)
            path.write_text('changed')
            with self.assertRaises(ValueError):require(path,digest)

    def test_archive_names_reject_escape_and_sensitive_build_configuration(self):
        validate=self.api('safe_archive_name')
        for name in ['../../outside','/absolute','src/../../../escape','.cargo/config.toml','build.rs','.git/config','C:/outside','C:relative','c:/outside','c:relative']:
            self.assertFalse(validate(name),name)
        self.assertTrue(validate('src/lib.rs'))
        self.assertTrue(validate('tests/behavior.rs'))

    def test_memory_skill_and_strategy_cases_do_not_claim_hooks_already_executed(self):
        for case in self.api('cases')():
            if case['family'] in ['Q09','Q10']:
                self.assertTrue(case.get('setup'))
                self.assertEqual(case['setup']['execution_status'],'not_executed')
                self.assertFalse(case['setup']['complete_scenario_proved'])

    def test_assertion_failures_required_for_mutation_kills(self):
        killed=self.api('mutation_killed')
        self.assertFalse(killed({'exit_code':101,'assertion_failure':False,'compiled':False}))
        self.assertFalse(killed({'exit_code':0,'assertion_failure':False,'compiled':True}))
        self.assertTrue(killed({'exit_code':101,'assertion_failure':True,'compiled':True}))

    def test_refactors_have_independent_structure_requirements(self):
        for case in self.api('cases')():
            if case['family']=='Q03':self.assertTrue(case.get('structure'))

    def test_quality_cases_fix_tests_keep_implementation_immutable_and_have_distinct_mutants(self):
        for case in self.api('cases')():
            if case['family']=='Q06':
                self.assertTrue(case.get('mutants'))
                self.assertNotIn('src/lib.rs',case['control'])
                self.assertIn('src/lib.rs',case['protected'])
                self.assertGreaterEqual(len(case['mutants']),3)

    @unittest.skipUnless(__import__('sys').platform=='linux','actual Linux bubblewrap oracle required')
    def test_failing_candidate_checks_cannot_be_masked_by_a_passing_external_contract(self):
        case=next(c for c in self.api('cases')() if c['id']=='Q02-H1')
        with tempfile.TemporaryDirectory() as directory:
            repo=Path(directory)/'candidate';self.api('materialize')(case,repo,True)
            (repo/'tests').mkdir();(repo/'tests/failing.rs').write_text('#[test]fn intentional_failure(){assert_eq!(1,2);}')
            sha=self.api('initialize')(repo,'Check-failure trap')
            receipt=self.api('evaluate')(case,repo,sha)
            self.assertEqual(receipt['candidate_checks']['exit_code'],101)
            self.assertEqual(receipt['behavior']['exit_code'],0)
            self.assertEqual(receipt['status'],'FAIL')

    @unittest.skipUnless(__import__('sys').platform=='linux','actual Linux bubblewrap oracle required')
    def test_oracle_sources_are_read_only_during_candidate_execution(self):
        case=next(c for c in self.api('cases')() if c['id']=='Q02-H1')
        with tempfile.TemporaryDirectory() as directory:
            repo=Path(directory)/'candidate';self.api('materialize')(case,repo,True)
            source=repo/'src/lib.rs'
            source.write_text(source.read_text().replace('pub fn ceil_div(value:u64,divisor:u64)->Option<u64>{',
                'pub fn ceil_div(value:u64,divisor:u64)->Option<u64>{assert!(std::fs::write("/sandbox/acceptance/tests/contract.rs","forged").is_err());'))
            sha=self.api('initialize')(repo,'Read-only oracle trap')
            receipt=self.api('evaluate')(case,repo,sha)
            self.assertEqual(receipt['status'],'PASS')

if __name__=='__main__':unittest.main(verbosity=2)

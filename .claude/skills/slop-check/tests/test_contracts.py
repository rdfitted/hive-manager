"""Verdict and reporting contracts; gate tests use the real hash-verified engine."""
import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
SKILL = HERE.parent
FIX = SKILL / 'fixtures'
sys.path.insert(0, str(SKILL / 'scripts'))
import slop_check


class Contracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def scan(self, *args):
        out = self.root / 'verdict.json'
        with contextlib.redirect_stdout(io.StringIO()):
            code = slop_check.main([*map(str, args), '--json', str(out), '--quiet'])
        return code, json.loads(out.read_text(encoding='utf-8'))

    def copy_tree(self, name):
        root = self.root / name
        (root / 'src').mkdir(parents=True)
        shutil.copy(FIX / 'side_tab.html', root / 'src' / 'panel.svelte')
        return root

    def baseline(self):
        base = self.copy_tree('base')
        code, verdict = self.scan(base / 'src' / 'panel.svelte', '--root', base)
        self.assertEqual(code, 2)
        path = self.root / 'baseline.json'
        path.write_text(json.dumps(verdict), encoding='utf-8')
        return path, verdict

    def test_baseline_subtracts_banned_findings_across_roots_and_line_movement(self):
        baseline, base = self.baseline()
        head = self.copy_tree('head')
        target = head / 'src' / 'panel.svelte'
        target.write_text('\n\n' + target.read_text(encoding='utf-8'), encoding='utf-8')
        code, verdict = self.scan(target, '--root', head, '--baseline', baseline)
        self.assertEqual((code, verdict['verdict']), (0, 'pass'))
        self.assertEqual(verdict['gating'], [])
        self.assertEqual(len(verdict['preexisting']), 1)
        finding = verdict['preexisting'][0]
        self.assertEqual(finding['banned'], 'ban-side-tab')
        self.assertEqual(finding['relpath'], 'src/panel.svelte')
        self.assertEqual(finding['finding_key'], base['gating'][0]['finding_key'])
        self.assertNotEqual(finding['line'], base['gating'][0]['line'])

    def test_new_finding_blocks_with_baseline(self):
        baseline, _ = self.baseline()
        head = self.copy_tree('head')
        target = head / 'src' / 'panel.svelte'
        with target.open('a', encoding='utf-8') as file:
            file.write('\n<style>.new{border-left:3px solid red;padding:20px}</style>')
        code, verdict = self.scan(target, '--root', head, '--baseline', baseline)
        self.assertEqual((code, verdict['verdict']), (2, 'block'))
        self.assertEqual(len(verdict['preexisting']), 1)
        self.assertEqual(len(verdict['gating']), 1)

    def test_context_is_source_bounded_and_relpath_is_explicit_root(self):
        head = self.copy_tree('head')
        target = head / 'src' / 'panel.svelte'
        text = target.read_text(encoding='utf-8').replace('Finding', 'Finding ' + 'a' * 600)
        target.write_text(text, encoding='utf-8')
        _, verdict = self.scan(target, '--root', head)
        finding = verdict['gating'][0]
        self.assertEqual(verdict['verdict_version'], 2)
        self.assertEqual(finding['relpath'], 'src/panel.svelte')
        self.assertEqual(len(finding['context']), 300)
        self.assertIn('border-left', finding['context'])
        self.assertTrue(Path(finding['file']).is_absolute())

    def test_outside_root_is_error(self):
        code, verdict = self.scan(FIX / 'clean.html', '--root', self.root)
        self.assertEqual((code, verdict['verdict']), (1, 'error'))

    def test_baseline_rejects_failed_or_incomplete_verdicts(self):
        for value in ({}, {'verdict_version': 2, 'verdict': 'error'},
                      {'verdict_version': 2, 'verdict': 'unavailable'},
                      {'verdict_version': 2, 'verdict': 'pass', 'errors': []}):
            with self.subTest(value=value):
                path = self.root / 'bad.json'
                path.write_text(json.dumps(value), encoding='utf-8')
                code, verdict = self.scan(FIX / 'clean.html', '--root', FIX, '--baseline', path)
                self.assertEqual((code, verdict['verdict']), (1, 'error'))

    def test_baseline_matches_all_buckets_and_normalizes_whitespace(self):
        path, base = self.baseline()
        finding = base['gating'].pop()
        finding['snippet'] = ' \n ' + finding['snippet'] + ' \t '
        base['suppressed'].append(finding)
        path.write_text(json.dumps(base), encoding='utf-8')
        head = self.copy_tree('head')
        code, verdict = self.scan(head / 'src' / 'panel.svelte', '--root', head, '--baseline', path)
        self.assertEqual((code, len(verdict['preexisting'])), (0, 1))

    def test_forged_baseline_key_is_error(self):
        path, base = self.baseline()
        base['gating'][0]['finding_key'] = 'forged'
        path.write_text(json.dumps(base), encoding='utf-8')
        code, verdict = self.scan(FIX / 'clean.html', '--root', FIX, '--baseline', path)
        self.assertEqual((code, verdict['verdict']), (1, 'error'))

    def logged_scan(self, target, mode, *extra):
        log = self.root / (mode + '.jsonl')
        code, verdict = self.scan(target, '--root', FIX, '--log', log,
                                  '--session', 'session-test', '--task', 'T5', '--mode', mode, *extra)
        return code, verdict, json.loads(log.read_text(encoding='utf-8').splitlines()[-1]), log

    def test_report_block_logs_task_counts_rules_without_source_content(self):
        code, verdict, row, _ = self.logged_scan(FIX / 'side_tab.html', 'report')
        self.assertEqual((code, verdict['verdict'], row['verdict']), (2, 'block', 'block'))
        self.assertEqual((row['session_id'], row['task_id'], row['mode']), ('session-test', 'T5', 'report'))
        self.assertEqual(row['counts']['gating'], 1)
        self.assertEqual(row['rules']['gating'], {'side-tab': 1})
        self.assertEqual(row['engine_version'], '0.1.5')
        self.assertTrue(Path(row['verdict_path']).is_absolute())
        self.assertNotIn('side_tab.html', json.dumps(row))
        self.assertNotIn('snippet', row)
        self.assertNotIn('border-left', json.dumps(row))
        self.assertEqual(set(row['findings']), {verdict['gating'][0]['finding_key']})

    def test_error_never_passes_in_any_mode(self):
        for mode in ('off', 'report', 'block'):
            with self.subTest(mode=mode):
                code, verdict, row, _ = self.logged_scan(FIX / 'does-not-exist.html', mode)
                self.assertEqual((code, verdict['verdict'], row['verdict']), (1, 'error', 'error'))
                self.assertTrue(verdict['errors'])

    def test_unavailable_never_passes_in_any_mode(self):
        # A missing override exercises the production engine failure path, not a stub.
        with patch.dict(os.environ, {'IMPECCABLE_BIN': str(self.root / 'missing.exe')}):
            for mode in ('off', 'report', 'block'):
                with self.subTest(mode=mode):
                    code, verdict, row, _ = self.logged_scan(FIX / 'clean.html', mode)
                    self.assertEqual((code, verdict['verdict'], row['verdict']), (1, 'unavailable', 'unavailable'))
                    self.assertTrue(verdict['errors'])

    def test_log_flags_are_all_or_none_and_log_path_absolute(self):
        for flags in (('--mode', 'report'), ('--log', 'relative.jsonl', '--session', 's', '--task', 't', '--mode', 'off')):
            code, verdict = self.scan(FIX / 'clean.html', '--root', FIX, *flags)
            self.assertEqual((code, verdict['verdict']), (1, 'error'))

    def test_log_failure_fails_command_and_verdict(self):
        code, verdict = self.scan(FIX / 'clean.html', '--root', FIX, '--log', self.root,
                                  '--session', 's', '--task', 't', '--mode', 'report')
        self.assertEqual((code, verdict['verdict']), (1, 'error'))
        self.assertIn('log:', verdict['errors'][0])

    def test_label_rates_cli_latest_label_wins_and_since_filters_runs(self):
        _, _, row, log = self.logged_scan(FIX / 'side_tab.html', 'report')
        key = next(iter(row['findings']))
        for label in ('fp', 'tp'):
            with contextlib.redirect_stdout(io.StringIO()):
                code = slop_check.main(['label', '--log', str(log), '--run', row['run_id'],
                                       '--finding', key, '--label', label, '--by', 'reviewer'])
            self.assertEqual(code, 0)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(slop_check.main(['rates', '--log', str(log)]), 0)
        self.assertEqual(json.loads(output.getvalue())['rules']['side-tab'],
                         {'hits': 1, 'labeled': 1, 'fp': 0, 'tp': 1, 'fp_rate': 0.0})
        self.assertEqual(slop_check.rates(slop_check.read_rows(str(log)), '9999-01-01'), {'rules': {}})

    def test_unknown_label_reference_is_rejected_without_append(self):
        _, _, row, log = self.logged_scan(FIX / 'side_tab.html', 'report')
        before = log.read_bytes()
        with contextlib.redirect_stderr(io.StringIO()):
            code = slop_check.main(['label', '--log', str(log), '--run', row['run_id'],
                                   '--finding', 'unknown', '--label', 'fp', '--by', 'reviewer'])
        self.assertEqual(code, 1)
        self.assertEqual(log.read_bytes(), before)

    def test_rates_counts_false_positives_and_joins_multiple_runs(self):
        _, _, first, log = self.logged_scan(FIX / 'side_tab.html', 'report')
        key = next(iter(first['findings']))
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(slop_check.main(['label', '--log', str(log), '--run', first['run_id'],
                                             '--finding', key, '--label', 'fp', '--by', 'reviewer']), 0)
        self.logged_scan(FIX / 'side_tab.html', 'report')
        result = slop_check.rates(slop_check.read_rows(str(log)), None)
        self.assertEqual(result['rules']['side-tab'],
                         {'hits': 2, 'labeled': 1, 'fp': 1, 'tp': 0, 'fp_rate': 1.0})

    def test_preexisting_and_suppressed_findings_cannot_be_labeled(self):
        baseline, _ = self.baseline()
        head = self.copy_tree('head')
        log = self.root / 'nonactionable.jsonl'
        code, verdict = self.scan(head / 'src' / 'panel.svelte', '--root', head,
                                  '--baseline', baseline, '--log', log,
                                  '--session', 's', '--task', 't', '--mode', 'report')
        self.assertEqual(code, 0)
        row = slop_check.read_rows(str(log))[0]
        self.assertEqual(row['findings'], {})
        self.assertEqual(row['rules']['preexisting'], {'side-tab': 1})
        self.assertEqual(slop_check.rates([row], None), {'rules': {}})
        _, _, suppressed, _ = self.logged_scan(FIX / 'spinner.tsx', 'off')
        self.assertEqual(suppressed['findings'], {})
        with contextlib.redirect_stderr(io.StringIO()):
            code = slop_check.main(['label', '--log', str(log), '--run', row['run_id'],
                                   '--finding', verdict['preexisting'][0]['finding_key'],
                                   '--label', 'fp', '--by', 'reviewer'])
        self.assertEqual(code, 1)

    def test_engine_malformed_or_empty_json_is_error(self):
        # Exercise parser failures only; successful fixture scans always use the real engine.
        for output in (b'', b'{invalid', b'{}'):
            with self.subTest(output=output), patch.object(slop_check.subprocess, 'run') as run:
                run.return_value = subprocess.CompletedProcess([], 0, output, b'')
                code, verdict = self.scan(FIX / 'clean.html', '--root', FIX)
                self.assertEqual((code, verdict['verdict']), (1, 'error'))

    def test_rates_excludes_nonactionable_hits_and_handles_unlabeled(self):
        _, _, _, log = self.logged_scan(FIX / 'side_tab.html', 'report')
        _, _, suppressed, other = self.logged_scan(FIX / 'spinner.tsx', 'off')
        self.assertEqual(suppressed['counts']['suppressed'], 1)
        result = slop_check.rates(slop_check.read_rows(str(self.root / '*.jsonl')), None)
        self.assertEqual(result['rules'], {'side-tab': {'hits': 1, 'labeled': 0, 'fp': 0, 'tp': 0, 'fp_rate': None}})
        self.assertEqual(len(slop_check.read_rows(str(log))), 1)
        self.assertEqual(len(slop_check.read_rows(str(other))), 1)

    def test_concurrent_process_appends_are_complete_unique_rows(self):
        log = self.root / 'concurrent.jsonl'
        program = ("import sys; sys.dont_write_bytecode=True; sys.path.insert(0,sys.argv[1]); "
                   "import slop_check; "
                   "[slop_check.append_row(sys.argv[2],{'writer':sys.argv[3],'n':n,'text':'x'*1000}) for n in range(100)]")
        processes = [subprocess.Popen([sys.executable, '-B', '-I', '-c', program,
                                       str(SKILL / 'scripts'), str(log), str(i)],
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE) for i in range(8)]
        for process in processes:
            _, error = process.communicate(timeout=60)
            self.assertEqual(process.returncode, 0, error.decode('utf-8'))
        rows = [json.loads(line) for line in log.read_text(encoding='utf-8').splitlines()]
        self.assertEqual(len(rows), 800)
        self.assertEqual(len({(r['writer'], r['n']) for r in rows}), 800)
        self.assertTrue(all(r['text'] == 'x' * 1000 for r in rows))

    def test_append_uses_one_write_and_short_write_fails(self):
        log = self.root / 'short.jsonl'
        with patch.object(slop_check.os, 'write', return_value=1) as write:
            with self.assertRaisesRegex(OSError, 'short log append'):
                slop_check.append_row(str(log), {'n': 1})
            self.assertEqual(write.call_count, 1)


if __name__ == '__main__':
    unittest.main()

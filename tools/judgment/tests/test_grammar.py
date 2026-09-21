import sys
import unittest
from pathlib import Path


JUDGMENT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(JUDGMENT_ROOT))

from plan_grammar import parse_plan_markdown_with_diagnostics, rust_lines  # noqa: E402


class PlanGrammarTests(unittest.TestCase):
    def test_rust_lines_handles_empty_trailing_and_crlf(self):
        self.assertEqual([], rust_lines(""))
        self.assertEqual(["one", "two"], rust_lines("one\r\ntwo\n"))
        self.assertEqual(["one", ""], rust_lines("one\n\n"))

    def test_bracket_tag_explicit_id_and_file_input(self):
        plan, diagnostics = parse_plan_markdown_with_diagnostics(
            "# Tagged plan\n\n## Tasks\n"
            "- [ ] [backend] T12: Parse plan [HIGH] (deps: T2) "
            "(inputs: file:src/parser.rs, artifact) (outputs: result) -> P1 Parser\n"
        )

        self.assertEqual([], diagnostics)
        self.assertEqual(1, len(plan.tasks))
        task = plan.tasks[0]
        self.assertEqual("T12", task.id)
        self.assertEqual("Parse plan", task.title)
        self.assertEqual(["T2"], task.depends_on)
        self.assertEqual(["file:src/parser.rs", "artifact"], task.inputs)
        self.assertEqual("high", task.priority)
        self.assertEqual("P1", task.assignee)
        self.assertEqual("Parser", task.assignee_label)

    def test_metadata_is_case_sensitive_except_tier(self):
        plan, diagnostics = parse_plan_markdown_with_diagnostics(
            "## Plan\n- [ ] T1: Work (Inputs: file:ignored.rs) (TIER: CRITICAL)\n"
        )
        self.assertEqual([], diagnostics)
        self.assertEqual([], plan.tasks[0].inputs)
        self.assertEqual("critical", plan.tasks[0].tier)
        self.assertIn("(Inputs: file:ignored.rs)", plan.tasks[0].title)

    def test_first_close_parenthesis_terminates_metadata(self):
        plan, diagnostics = parse_plan_markdown_with_diagnostics(
            "## Tasks\n- [ ] T1: Work (acceptance: one (nested), two)\n"
        )
        self.assertEqual([], diagnostics)
        self.assertEqual(["one (nested"], plan.tasks[0].acceptance)
        self.assertEqual("Work , two)", plan.tasks[0].title)

    def test_explicit_graph_filters_descriptive_bullets_and_diagnoses_checkbox(self):
        plan, diagnostics = parse_plan_markdown_with_diagnostics(
            "## Tasks\n- prose retained by legacy parser\n- [ ] Missing stable id\n- [ ] T2: Stable\n"
        )
        self.assertEqual(["task-1", "task-2", "T2"], [task.id for task in plan.tasks])
        self.assertEqual(1, len(diagnostics))
        self.assertIn("Missing stable id", diagnostics[0])

    def test_unterminated_metadata_preserves_original_text_and_defaults(self):
        plan, diagnostics = parse_plan_markdown_with_diagnostics(
            "## Tasks\n- [ ] T1: Work (inputs: file:src/x.rs\n"
        )
        task = plan.tasks[0]
        self.assertEqual([], task.inputs)
        self.assertEqual("Work (inputs: file:src/x.rs", task.title)
        self.assertEqual(["line 2: unterminated (inputs: ...) metadata"], diagnostics)


if __name__ == "__main__":
    unittest.main()

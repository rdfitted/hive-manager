"""Known names from a local memory tree, and the checks that block on them. All names are synthetic."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

import redaction  # noqa: E402


CLIENT_PROFILE = """# Zebulon Holdings

- **Name:** Zebulon Holdings Group (the "Quillfeather" account)

| Name | Email | Role |
|---|---|---|
| **Ottoline Vandersnoot** | hidden | Owner |
| **David ("Davy") Quillon** | hidden | Finance |
| **Finance** | hidden | Shared inbox |
| **The Zarnow Collective** | hidden | Venue |

| Role | State |
|---|---|
| **First operational team** | Live |
| **The referral source** | Past |
"""


def blocked(text, dictionary):
    value, report = redaction.redact({"text": text}, "names", dictionary)
    return value is None and report["blocked"]


class MemoryTreeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        client = self.root / "clients" / "zebulon-holdings"
        client.mkdir(parents=True)
        (client / "CLIENT.md").write_text(CLIENT_PROFILE, encoding="utf-8")
        (client / "PROFILE.md").write_text("  | Name | Email |\n  |---|---|\n  | **Perpetua Ostrander** | hidden |\n",
                                           encoding="utf-8")
        (client / "context.md").write_text("| Name |\n|---|\n| **Decoy Context** |\n", encoding="utf-8")
        (self.root / "clients" / ".git").mkdir()
        (self.root / "clients" / "CLIENTS.md").write_text("- **[Zebulon Holdings](zebulon-holdings/)** note", encoding="utf-8")
        (self.root / "partners" / "ignatius-pemberly").mkdir(parents=True)
        pipeline = self.root / "pipeline" / "complete-widget-works"
        pipeline.mkdir(parents=True)
        (pipeline / "PROFILE.md").write_text("- **Name:** Complete Widget Works Corp.\n", encoding="utf-8")
        memory = self.root / "projects" / "some-project" / "memory"
        memory.mkdir(parents=True)
        (memory / "person_wilhelmina_tarn.md").write_text("synthetic", encoding="utf-8")
        self.dictionary = redaction.load_dictionary(None, None, self.root)
        self.terms = {entry["term"] for entry in self.dictionary}

    def test_every_source_is_covered(self):
        for text in ("Zebulon Holdings", "zebulon-holdings", "zebulon", "zebulonbrain", "ZebulonHoldings",
                     "Quillfeather", "Ottoline Vandersnoot", "Vandersnoot", "Ostrander", "Quillon", "Davy",
                     "Ignatius Pemberly", "Pemberly", "complete-widget-works", "Complete Widget Works Corp.",
                     "Wilhelmina Tarn", "Tarn"):
            with self.subTest(text=text):
                self.assertTrue(blocked(f"Seen in {text} today.", self.dictionary))

    def test_ordinary_words_are_not_names(self):
        for word in ("The", "First", "Finance", "Group", "Complete", "Widget"):
            with self.subTest(word=word):
                self.assertNotIn(word, self.terms)
        self.assertFalse(blocked("The first complete widget group shipped. The finance report ran.",
                                 self.dictionary))

    def test_only_tables_listing_people_give_people(self):
        self.assertNotIn("First operational team", self.terms)
        self.assertNotIn("Decoy Context", self.terms)

    def test_a_prospects_first_word_alone_is_not_a_name(self):
        self.assertFalse(blocked("The task is complete.", self.dictionary))
        self.assertTrue(blocked("Work for complete-widget-works.", self.dictionary))

    def test_a_dot_directory_is_not_a_client(self):
        self.assertNotIn(".git", self.terms)
        self.assertFalse(blocked("Read .git/config at the head.", self.dictionary))
        (self.root / "profiles" / ".git").mkdir(parents=True)
        (self.root / "profiles" / "real-name").mkdir()
        self.assertEqual(["real name"], redaction.load_dictionary(None, self.root / "profiles"))

    def test_names_inside_identifiers_are_caught(self):
        for text in ("zebulon_orders", "orders_zebulon", "zebulon2", "ZEBULON_API_URL", "src/zebulon/mod.rs"):
            with self.subTest(text=text):
                self.assertTrue(blocked(text, self.dictionary))
        self.assertFalse(blocked("zebulons are a plural, and subzebulon isn't the name.", self.dictionary))

    def test_short_person_names_stay_case_sensitive(self):
        dictionary = [{"term": "Will", "kind": "person"}]
        self.assertTrue(blocked("Will said so", dictionary))
        self.assertFalse(blocked("it will run", dictionary))

    def test_a_missing_memory_root_fails_closed(self):
        with self.assertRaises(ValueError):
            redaction.load_dictionary(None, None, self.root / "absent")
        empty = self.root / "empty"
        empty.mkdir()
        with self.assertRaises(ValueError):
            redaction.load_dictionary(None, None, empty)

    def test_the_report_never_echoes_a_match(self):
        _, report = redaction.redact({"text": "Ottoline Vandersnoot, +1 555-010-9999"}, "names", self.dictionary)
        self.assertEqual(["dictionary", "phone"], report["reason_classes"])
        self.assertNotIn("Vandersnoot", str(report))


class ContactDetailTests(unittest.TestCase):
    DICTIONARY = ["Synthetic Unused Name"]

    def test_phone_numbers_block_and_bare_digit_runs_do_not(self):
        for text in ("Call 555-010-9999", "(555) 010-9999", "+1 555.010.9999"):
            with self.subTest(text=text):
                self.assertTrue(blocked(text, self.DICTIONARY))
        for text in ("timestamp 1793382973", "took 412 ms at 1.5 s", "version 0.60.2", "sha 5550109999ab"):
            with self.subTest(text=text):
                self.assertFalse(blocked(text, self.DICTIONARY))

    def test_urls_block_unless_the_host_identifies_no_one(self):
        for text in ("https://portal.somecompany.co/login", "see http://10.1.2.3:8600/x"):
            with self.subTest(text=text):
                self.assertTrue(blocked(text, self.DICTIONARY))
        for text in ("curl http://localhost:8600/api", "http://127.0.0.1:18800/x", "http://[::1]:8600/api", "https://api.example.com/v1",
                     "https://github.com/synthetic/repo/pull/1", "Origin http://other.test"):
            with self.subTest(text=text):
                self.assertFalse(blocked(text, self.DICTIONARY))

    def test_a_safe_host_does_not_hide_a_name_in_its_path(self):
        self.assertTrue(blocked("https://github.com/acme/zebulon-repo", ["zebulon-repo"]))


if __name__ == "__main__":
    unittest.main()

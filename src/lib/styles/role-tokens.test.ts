import { describe, expect, it, vi } from 'vitest';

// Read production sources without the Vite CSS stubs or live user settings.
const { readFileSync, readdirSync, statSync } = await vi.importActual<{
  readFileSync(path: string, encoding: 'utf8'): string;
  readdirSync(path: string): string[];
  statSync(path: string): { isDirectory(): boolean };
}>('node:fs');

type Allowance = { reason: string; occurrences: Record<string, number> };

// Exact occurrence budgets stop a new use even in a file awaiting migration.
// The follow-up primitive lane removes migrated entries, retaining only reasoned
// terminal palette reads. A reason never grants a whole-file exemption.
const hexAllowlist: Record<string, Allowance> = {
  "src/lib/components/ResumeConfirmModal.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "#787c99": 1,
      "#e0af68": 1,
      "#f7768e": 1
    }
  },
  "src/lib/components/SessionSidebar.svelte": {
    "reason": "PC T13 shell/navigation migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "#7aa2f7": 1,
      "#7dcfff": 1,
      "#9ece6a": 1,
      "#bb9af7": 1,
      "#e0af68": 1,
      "#f7768e": 1,
      "#f7b1d1": 1,
      "#ff9e64": 1
    }
  }
};

const roleAllowlist: Record<string, Allowance> = {
  "src/lib/components/AddWorkerDialog.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 2
    }
  },
  "src/lib/components/agent/AgentList.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/AgentConfigEditor.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 3
    }
  },
  "src/lib/components/AgentStatusBar.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--bg-void": 1,
      "--text-secondary": 3
    }
  },
  "src/lib/components/AgentTree.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/artifacts/ArtifactSummary.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 3
    }
  },
  "src/lib/components/artifacts/DiffSummaryPanel.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 2
    }
  },
  "src/lib/components/artifacts/TestResultsPanel.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 2
    }
  },
  "src/lib/components/attention/AttentionCard.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/attention/FlushReceipt.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/BranchSelector.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 2
    }
  },
  "src/lib/components/chat/ActivityGroup.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 2
    }
  },
  "src/lib/components/chat/ChatPane.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 3
    }
  },
  "src/lib/components/composer/Composer.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 3,
      "--accent-cyan": 4,
      "--bg-void": 1,
      "--text-secondary": 1
    }
  },
  "src/lib/components/composer/ComposerToolbar.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--bg-void": 1,
      "--text-secondary": 2
    }
  },
  "src/lib/components/composer/MentionMenu.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 1
    }
  },
  "src/lib/components/composer/SlashMenu.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 1
    }
  },
  "src/lib/components/ContractViewer.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--bg-void": 2,
      "--text-secondary": 3
    }
  },
  "src/lib/components/ConversationViewer.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 2,
      "--accent-cyan": 1,
      "--bg-void": 1,
      "--text-secondary": 2
    }
  },
  "src/lib/components/CoordinationPanel.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 1,
      "--accent-cyan": 1,
      "--bg-void": 1,
      "--text-secondary": 6
    }
  },
  "src/lib/components/dashboard/KanbanBoard.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 1,
      "--status-queued": 1
    }
  },
  "src/lib/components/dashboard/KanbanCard.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 2
    }
  },
  "src/lib/components/dashboard/KanbanColumn.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/DebatePanel.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 5,
      "--bg-void": 5,
      "--text-secondary": 4
    }
  },
  "src/lib/components/fusion/FusionComparisonView.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/fusion/ResolverPanel.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 3,
      "--bg-void": 1,
      "--text-secondary": 3
    }
  },
  "src/lib/components/FusionPanel.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--bg-void": 9,
      "--text-secondary": 1
    }
  },
  "src/lib/components/knowledge/KnowledgeGraph.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--bg-void": 4
    }
  },
  "src/lib/components/knowledge/KnowledgePagePreview.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 3,
      "--accent-cyan": 2,
      "--bg-void": 1,
      "--text-secondary": 4
    }
  },
  "src/lib/components/knowledge/KnowledgeTable.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 3,
      "--bg-void": 1,
      "--text-secondary": 1
    }
  },
  "src/lib/components/knowledge/MarkdownInline.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 4
    }
  },
  "src/lib/components/knowledge/WikiSettings.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/launch/LaunchComposer.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 2,
      "--bg-void": 2,
      "--text-secondary": 3
    }
  },
  "src/lib/components/palette/CommandPalette.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 2
    }
  },
  "src/lib/components/PlanView.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 4,
      "--text-secondary": 10
    }
  },
  "src/lib/components/QaFeedbackPanel.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 6
    }
  },
  "src/lib/components/QueenControls.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/renderers/Approval.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/renderers/Chart.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 1,
      "--accent-cyan": 2,
      "--text-secondary": 3
    }
  },
  "src/lib/components/renderers/DataTable.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 1,
      "--accent-cyan": 2,
      "--text-secondary": 2
    }
  },
  "src/lib/components/renderers/Diff.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 3
    }
  },
  "src/lib/components/renderers/ToolRenderHost.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/replay/ReplayControls.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 2
    }
  },
  "src/lib/components/replay/ReplayView.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--bg-void": 1,
      "--text-secondary": 1
    }
  },
  "src/lib/components/ResizeHandle.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1
    }
  },
  "src/lib/components/ResumeConfirmModal.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/review/ReviewView.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/session/SessionHeader.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 3,
      "--bg-void": 5,
      "--text-secondary": 3
    }
  },
  "src/lib/components/session/SessionOverview.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--bg-void": 2,
      "--text-secondary": 1
    }
  },
  "src/lib/components/SessionFilesView.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 1,
      "--bg-void": 2,
      "--text-secondary": 2
    }
  },
  "src/lib/components/SessionSidebar.svelte": {
    "reason": "PC T13 shell/navigation migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 3,
      "--bg-void": 4,
      "--text-secondary": 6
    }
  },
  "src/lib/components/shell/TitleBar.svelte": {
    "reason": "PC T13 shell/navigation migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--bg-void": 1,
      "--text-secondary": 1
    }
  },
  "src/lib/components/ShortcutsOverlay.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--bg-void": 1,
      "--text-secondary": 3
    }
  },
  "src/lib/components/StatusPanel.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 7
    }
  },
  "src/lib/components/templates/TemplateEditor.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 3,
      "--text-secondary": 1
    }
  },
  "src/lib/components/templates/TemplatePicker.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 3,
      "--bg-void": 1,
      "--text-secondary": 3
    }
  },
  "src/lib/components/Terminal.svelte": {
    "reason": "Terminal palette reads must retain D1 literals; UI frame uses are reviewed separately by PD1 and PB2.",
    "occurrences": {
      "--accent-cyan": 3,
      "--bg-void": 4,
      "--text-secondary": 5
    }
  },
  "src/lib/components/TerminalGrid.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 5
    }
  },
  "src/lib/components/timeline/EventDetailsModal.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--bg-void": 1
    }
  },
  "src/lib/components/timeline/EventListItem.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1
    }
  },
  "src/lib/components/TreeItem.svelte": {
    "reason": "PC T13 shell/navigation migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 4,
      "--bg-void": 1,
      "--text-secondary": 1
    }
  },
  "src/lib/components/UpdateChecker.svelte": {
    "reason": "PD1 T14 workspace/composer/dialog migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 4
    }
  },
  "src/lib/components/workgraph/NodeInspector.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 3,
      "--text-secondary": 4
    }
  },
  "src/lib/components/workgraph/ProgressHeader.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/lib/components/workgraph/WorkGraphView.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 9,
      "--text-secondary": 8
    }
  },
  "src/routes/+page.svelte": {
    "reason": "PC T13 shell/navigation migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-cyan": 1,
      "--text-secondary": 3
    }
  },
  "src/routes/dashboard/+page.svelte": {
    "reason": "PD2 T15 orchestration migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--text-secondary": 1
    }
  },
  "src/routes/knowledge/+page.svelte": {
    "reason": "PD3 T16 content/multi-agent migration pending; PB2 T17 removes this allowance.",
    "occurrences": {
      "--accent-amber": 2,
      "--accent-cyan": 9,
      "--bg-void": 2,
      "--text-secondary": 5
    }
  }
};

function componentFiles(directory: string): string[] {
  return readdirSync(directory).flatMap((name) => {
    const path = `${directory}/${name}`;
    return statSync(path).isDirectory() ? componentFiles(path) : path.endsWith('.svelte') ? [path] : [];
  });
}

function violations(pattern: RegExp, allowlist: Record<string, Allowance>): string[] {
  return componentFiles('src').flatMap((path) => {
    const counts: Record<string, number> = {};
    for (const match of readFileSync(path, 'utf8').matchAll(pattern)) {
      counts[match[0]] = (counts[match[0]] ?? 0) + 1;
    }
    return Object.entries(counts).flatMap(([value, count]) => {
      const budget = allowlist[path]?.occurrences[value] ?? 0;
      return count > budget ? [`${path}: ${value} has ${count} occurrences; allowed ${budget}`] : [];
    });
  });
}

describe('UI color migration guards', () => {
  it('requires a specific reason and occurrence budget for every pending file', () => {
    for (const allowance of [...Object.values(hexAllowlist), ...Object.values(roleAllowlist)]) {
      expect(allowance.reason.trim().length).toBeGreaterThan(30);
      expect(Object.keys(allowance.occurrences).length).toBeGreaterThan(0);
      for (const count of Object.values(allowance.occurrences)) expect(count).toBeGreaterThan(0);
    }
  });

  it('rejects new six-digit hex colors in components (G26)', () => {
    expect(violations(/#[0-9a-f]{6}\b/gi, hexAllowlist)).toEqual([]);
  });

  it('rejects new legacy UI token uses in components (G28)', () => {
    expect(violations(/--(?:bg-void|accent-cyan|accent-amber|text-secondary|status-queued|status-canceled)\b/g, roleAllowlist)).toEqual([]);
  });

  it('keeps global CSS migrated without any legacy UI roles', () => {
    for (const name of readdirSync('src/lib/styles/globals').filter((name) => name.endsWith('.css'))) {
      expect(readFileSync(`src/lib/styles/globals/${name}`, 'utf8'), name)
        .not.toMatch(/--(?:bg-void|accent-cyan|accent-amber|text-secondary|status-queued|status-canceled)\b/);
    }
  });
});

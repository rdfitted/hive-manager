import { beforeEach, describe, it, expect, vi } from 'vitest';
import { sessions } from './sessions';
import { invoke } from '@tauri-apps/api/core';

// Mock Tauri invoke
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

// Mock Tauri listen
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

describe('sessions store', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (invoke as ReturnType<typeof vi.fn>).mockResolvedValue({
      id: 'session-test-id',
      agents: [],
    });
  });

  describe('launchSolo', () => {
    it('sends with_evaluator and evaluator_config correctly in the payload', async () => {
      const config = {
        projectPath: '/test/path',
        cli: 'claude',
        taskDescription: 'test task',
        with_evaluator: true,
        evaluator_config: {
          cli: 'gemini',
          flags: ['--test'],
          label: 'Test Evaluator'
        },
        qa_workers: [
          { specialization: 'ui', cli: 'claude', flags: [] }
        ]
      };

      await sessions.launchSolo(config as any);

      expect(invoke).toHaveBeenCalledWith('launch_hive_v2', expect.objectContaining({
        config: expect.objectContaining({
          with_evaluator: true,
          evaluator_config: expect.objectContaining({
            cli: 'gemini',
            label: 'Test Evaluator'
          }),
          qa_workers: expect.arrayContaining([
            expect.objectContaining({ specialization: 'ui' })
          ])
        })
      }));
    });

    it('handles solo launch without evaluator', async () => {
      const config = {
        projectPath: '/test/path',
        cli: 'claude',
        taskDescription: 'test task',
        with_evaluator: false
      };

      await sessions.launchSolo(config as any);

      expect(invoke).toHaveBeenCalledWith('launch_hive_v2', expect.objectContaining({
        config: expect.objectContaining({
          with_evaluator: false,
          evaluator_config: undefined,
          qa_workers: undefined
        })
      }));
    });
  });
});

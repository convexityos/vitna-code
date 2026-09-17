// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ApprovalView } from '../run/model';
import { ApprovalBox } from './ApprovalBox';

const DIGEST = 'ab'.repeat(32);

function approval(status: ApprovalView['status'] = 'pending'): ApprovalView {
  return {
    request: {
      approval_id: 'a1',
      tool_call_id: 't1',
      action_digest: DIGEST,
      description: 'Write 13 added lines to src/service.py',
      executable_identity: 'vitna-runner apply_patch 1.0.0',
      canonical_cwd: '/w',
      environment_names: [],
      bound_mounts: ['/w/src (read-write)'],
      timeout_ms: 0,
    },
    runId: 'r1',
    sequence: 4,
    receivedAt: 0,
    status,
    choice: status === 'pending' ? null : 'approve',
    scope: status === 'pending' ? null : 'once',
    reason: '',
    message: '',
  };
}

function mount(view: ApprovalView, connected = true) {
  const actions = { approve: vi.fn(async () => {}), reject: vi.fn(async () => {}) };
  const utils = render(<ApprovalBox approval={view} tool={null} diff={null} connected={connected} actions={actions} />);
  const box = screen.getByRole('region', { name: /Write 13 added lines/ });
  return { ...utils, actions, box };
}

afterEach(cleanup);

describe('ApprovalBox keys', () => {
  it('takes focus when nothing else has it, and y approves once', () => {
    const { actions, box } = mount(approval());
    expect(document.activeElement).toBe(box);
    fireEvent.keyDown(box, { key: 'y' });
    expect(actions.approve).toHaveBeenCalledWith('a1', 'once');
    expect(actions.reject).not.toHaveBeenCalled();
  });

  it('leaves focus with someone who is typing', () => {
    const area = document.createElement('textarea');
    area.value = 'half a thought';
    document.body.appendChild(area);
    area.focus();
    const { box } = mount(approval());
    expect(document.activeElement).toBe(area);
    expect(document.activeElement).not.toBe(box);
    area.remove();
  });

  it('maps the digits to the numbered choices', () => {
    const { actions, box } = mount(approval());
    fireEvent.keyDown(box, { key: '2' });
    expect(actions.approve).toHaveBeenLastCalledWith('a1', 'session');
    fireEvent.keyDown(box, { key: '3' });
    expect(actions.approve).toHaveBeenLastCalledWith('a1', 'project');
    fireEvent.keyDown(box, { key: '4' });
    expect(actions.reject).toHaveBeenLastCalledWith('a1', '');
  });

  it('ignores a held key and a chorded key', () => {
    const { actions, box } = mount(approval());
    fireEvent.keyDown(box, { key: 'y', repeat: true });
    fireEvent.keyDown(box, { key: 'y', ctrlKey: true });
    fireEvent.keyDown(box, { key: '1', metaKey: true });
    expect(actions.approve).not.toHaveBeenCalled();
  });

  it('moves the cursor with the arrows and chooses with enter', () => {
    const { actions, box } = mount(approval());
    fireEvent.keyDown(box, { key: 'ArrowDown' });
    fireEvent.keyDown(box, { key: 'ArrowDown' });
    fireEvent.keyDown(box, { key: 'ArrowDown' });
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(actions.reject).toHaveBeenCalledWith('a1', '');
    expect(actions.approve).not.toHaveBeenCalled();
  });

  it('sends a reason with the fifth choice', () => {
    const { actions, box } = mount(approval());
    fireEvent.keyDown(box, { key: '5' });
    const input = screen.getByLabelText(/reason/);
    fireEvent.change(input, { target: { value: 'not on this branch' } });
    fireEvent.keyDown(input, { key: 'y' });
    expect(actions.approve).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(actions.reject).toHaveBeenCalledWith('a1', 'not on this branch');
  });

  it('sends nothing when not connected', () => {
    const { actions, box } = mount(approval(), false);
    fireEvent.keyDown(box, { key: 'y' });
    fireEvent.keyDown(box, { key: '4' });
    expect(actions.approve).not.toHaveBeenCalled();
    expect(actions.reject).not.toHaveBeenCalled();
    expect(screen.getByText(/Not connected to the daemon/)).toBeInTheDocument();
  });

  it('has no choices left once a decision has gone out', () => {
    const actions = { approve: vi.fn(async () => {}), reject: vi.fn(async () => {}) };
    const { container } = render(
      <ApprovalBox approval={approval('sent')} tool={null} diff={null} connected actions={actions} />,
    );
    expect(screen.queryByRole('region')).toBeNull();
    expect(container.textContent).toMatch(/approved once · waiting for the daemon to start it/);
    fireEvent.keyDown(container.firstElementChild as Element, { key: 'n' });
    fireEvent.keyDown(document.body, { key: 'n' });
    expect(actions.reject).not.toHaveBeenCalled();
  });

  it('prints the digest in full and names an outcome the daemon refused', () => {
    mount(approval());
    expect(screen.getByLabelText(`Action digest ${DIGEST}`)).toBeInTheDocument();
    cleanup();
    const refused = { ...approval('refused'), choice: 'approve' as const, message: 'digest mismatch' };
    const { container } = render(
      <ApprovalBox approval={refused} tool={null} diff={null} connected actions={{ approve: vi.fn(), reject: vi.fn() }} />,
    );
    expect(container.textContent).toMatch(/the daemon refused your approval: digest mismatch/);
  });
});

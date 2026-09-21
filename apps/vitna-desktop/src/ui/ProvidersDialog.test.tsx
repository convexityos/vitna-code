// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CATALOG } from '../catalog/catalog';
import type { HostCapabilities, HostProvider } from '../transport/types';
import { ProvidersDialog } from './ProvidersDialog';

const ANTHROPIC: HostProvider = {
  id: 'anthropic',
  name: 'Anthropic',
  description: 'Claude models, called directly',
  credentialSource: 'environment variable ANTHROPIC_API_KEY',
};

/** A provider a host may well offer that the pinned catalog does not price. */
const UNKNOWN: HostProvider = {
  id: 'some-gateway',
  name: 'Some Gateway',
  description: 'Whatever the host has wired up',
  credentialSource: null,
};

function mount(providers: HostProvider[]) {
  const host: HostCapabilities = { listProviders: vi.fn(async () => providers) };
  return render(<ProvidersDialog host={host} onClose={vi.fn()} />);
}

afterEach(cleanup);

describe('the providers dialog', () => {
  it('counts a provider’s models without listing them until asked', async () => {
    mount([ANTHROPIC]);
    const toggle = await screen.findByRole('button', { name: /models$/ });

    const anthropic = CATALOG.providers.find((p) => p.id === 'anthropic');
    expect(toggle.textContent).toContain(`${anthropic?.models.length} models`);
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText('claude-opus-5')).toBeNull();

    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('claude-opus-5')).toBeTruthy();
  });

  it('prints a rate and a context window beside each model', async () => {
    mount([ANTHROPIC]);
    fireEvent.click(await screen.findByRole('button', { name: /models$/ }));

    const sku = screen.getByText('claude-opus-5');
    const facts = sku.parentElement?.querySelector('.model-facts')?.textContent ?? '';
    // The unit travels with the number. A row reading "1M ctx · 5 / 25" is a
    // row somebody will one day read as dollars per call.
    expect(facts).toMatch(/^1M ctx · \$\d/);
    expect(facts).toContain('per Mtok');
  });

  it('keeps one provider open while another is opened in the same tick', async () => {
    // Both clicks go inside one act() rather than through two fireEvent calls,
    // because fireEvent flushes a render between them and so cannot see this
    // bug at all: the handler has to spread the CURRENT map, not the one its
    // own render closed over, or the second click drops the first provider.
    // Found by driving the real page from a script, where React batched the
    // two clicks exactly like this.
    mount([ANTHROPIC, { ...ANTHROPIC, id: 'openai', name: 'OpenAI' }]);
    const toggles = await screen.findAllByRole('button', { name: /models$/ });

    act(() => {
      (toggles[0] as HTMLElement).click();
      (toggles[1] as HTMLElement).click();
    });

    expect(toggles[0]?.getAttribute('aria-expanded')).toBe('true');
    expect(toggles[1]?.getAttribute('aria-expanded')).toBe('true');
  });

  it('says outright that it cannot price a provider it does not carry', async () => {
    // The failure this replaces is silence: a provider name with an empty list
    // under it, which reads as a provider that sells nothing rather than one
    // this snapshot has never heard of.
    mount([UNKNOWN]);
    expect(await screen.findByText(/not in the pinned catalog/i)).toBeTruthy();
    expect(screen.queryByRole('button', { name: /models$/ })).toBeNull();
  });

  it('finds a provider by a model name it sells', async () => {
    mount([ANTHROPIC, UNKNOWN]);
    await screen.findByText('Anthropic');

    fireEvent.change(screen.getByPlaceholderText('Search providers'), { target: { value: 'opus' } });

    // The search reaches the catalog, opens the match, and drops the provider
    // that sells nothing by that name.
    expect(screen.getByText('claude-opus-5')).toBeTruthy();
    expect(screen.queryByText('Some Gateway')).toBeNull();
  });

  it('dates the prices it is showing', async () => {
    mount([ANTHROPIC]);
    expect(await screen.findByText(new RegExp(`taken ${CATALOG.fetched_at}`))).toBeTruthy();
    expect(screen.getByText(/never fetches them/)).toBeTruthy();
  });
});

import type { PlanChanged } from '../protocol/types';

interface PlanListProps {
  plan: PlanChanged;
  waiting: boolean;
  finished: boolean;
}

/**
 * The plan as the daemon last stated it, as a checklist in the transcript:
 * a tick for what is done, the prompt glyph for where the run stands, and
 * nothing for what is still to come. When the active step is waiting on the
 * operator the glyph turns rust and says so in words.
 */
export function PlanList({ plan, waiting, finished }: PlanListProps) {
  if (plan.steps.length === 0) return null;
  const active = Math.min(plan.active_step_index, plan.steps.length - 1);
  return (
    <ol className="plan" aria-label="Plan">
      {plan.steps.map((step, i) => {
        const state = finished || i < active ? 'done' : i === active ? (waiting ? 'waiting' : 'active') : 'next';
        return (
          <li key={`${i}:${step}`} className="plan-step" data-state={state} aria-current={state === 'active' || state === 'waiting' ? 'step' : undefined}>
            <span className="plan-glyph" aria-hidden="true">
              {state === 'done' ? '✓' : state === 'next' ? '' : '❯'}
            </span>
            <span className="plan-folio">{String(i + 1).padStart(2, '0')}</span>
            <span className="plan-label">{step}</span>
            {state === 'waiting' ? <span className="plan-wait">waiting on you</span> : null}
            {state === 'done' ? <span className="visually-hidden">(done)</span> : null}
          </li>
        );
      })}
    </ol>
  );
}

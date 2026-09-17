import { useEffect, useId, useMemo, useState, type DragEvent } from 'react';
import { loadSample } from '../app/transportChoice';
import { EVIDENCE_GRADES, readReceipt, type Receipt } from '../receipt/receipt';
import { checkDeviceSignature, signatureShape, type SignatureCheck } from '../receipt/verify';
import { shortDigest } from './text';
import { completionWord, gradeWord, isolationWord } from './words';

interface ReceiptViewProps {
  isSample: boolean;
}

const MAX_RECEIPT_BYTES = 4 * 1024 * 1024;

/**
 * Reads a vitna-run-receipt-v1 file and says, check by check, what this
 * window could and could not establish about it. Nothing is sent anywhere:
 * the file is read in the page and the signature is checked with WebCrypto.
 */
export function ReceiptView({ isSample }: ReceiptViewProps) {
  const [text, setText] = useState('');
  const [source, setSource] = useState('');
  const [key, setKey] = useState('');
  const [fileError, setFileError] = useState('');
  const [signature, setSignature] = useState<SignatureCheck | null>(null);
  const [dragging, setDragging] = useState(false);
  const [sampleNote, setSampleNote] = useState('');
  const inputId = useId();
  const keyId = useId();

  const reading = useMemo(() => (text.trim() ? readReceipt(text) : null), [text]);
  const receipt = reading?.receipt ?? null;

  useEffect(() => {
    let live = true;
    if (!receipt) {
      setSignature(null);
      return undefined;
    }
    void checkDeviceSignature(receipt, key).then((result) => {
      if (live) setSignature(result);
    });
    return () => {
      live = false;
    };
  }, [receipt, key]);

  const loadFile = async (file: File) => {
    setFileError('');
    if (file.size > MAX_RECEIPT_BYTES) {
      setFileError(`${file.name} is ${Math.round(file.size / 1024)} KB, larger than any receipt this reader opens.`);
      return;
    }
    setText(await file.text());
    setSource(file.name);
  };

  const onDrop = (event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    setDragging(false);
    const file = event.dataTransfer.files[0];
    if (file) void loadFile(file);
  };

  const openSampleReceipt = async () => {
    if (!loadSample) return;
    const sample = await loadSample();
    const built = await sample.buildSampleReceipt();
    if (!built.ok) {
      setSampleNote(built.reason);
      return;
    }
    setText(built.json);
    setKey(built.publicKeyHex);
    setSource('sample receipt, signed in this window a moment ago');
    setSampleNote('Edit any signed field in the text and the signature check fails.');
  };

  return (
    <div className="receipt-view">
      <header className="receipt-head">
        <p className="eyebrow">vitna-run-receipt-v1</p>
        <h1 className="receipt-title">Receipt reader</h1>
        <p className="receipt-lede">
          Open a receipt exported by <code>vitna receipt export</code>. It is read in this window and checked here; nothing
          is uploaded.
        </p>
      </header>

      <div className="receipt-grid">
        <section className="receipt-input" aria-label="Receipt">
          <label
            className="dropzone"
            data-dragging={dragging || undefined}
            htmlFor={inputId}
            onDragOver={(e) => {
              e.preventDefault();
              setDragging(true);
            }}
            onDragLeave={() => setDragging(false)}
            onDrop={onDrop}
          >
            <span className="dropzone-title">Drop a receipt here, or choose a file</span>
            <span className="mono fainter">{source || 'JSON, up to 4 MB'}</span>
            <input
              id={inputId}
              type="file"
              accept="application/json,.json"
              className="visually-hidden"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file) void loadFile(file);
                e.target.value = '';
              }}
            />
          </label>
          {fileError ? <p className="tone-rust">{fileError}</p> : null}

          <label className="receipt-label">
            <span className="eyebrow">Receipt text</span>
            <textarea
              className="field mono receipt-text"
              value={text}
              spellCheck={false}
              placeholder="Or paste the JSON here."
              onChange={(e) => {
                setText(e.target.value);
                if (!source.startsWith('sample')) setSource('pasted text');
              }}
            />
          </label>

          <label className="receipt-label" htmlFor={keyId}>
            <span className="eyebrow">Device public key</span>
          </label>
          <input
            id={keyId}
            className="field mono"
            value={key}
            spellCheck={false}
            placeholder="64 hex characters. Without it the signature is not checked."
            onChange={(e) => setKey(e.target.value)}
          />

          {loadSample ? (
            <div className="receipt-sample">
              <button type="button" className="pill pill-small" onClick={() => void openSampleReceipt()}>
                Load the sample receipt
              </button>
              <span className="faint">
                Development only{isSample ? ', for the sample session' : ''}. {sampleNote}
              </span>
            </div>
          ) : null}
        </section>

        <section className="receipt-findings" aria-label="Findings" aria-live="polite">
          {!reading ? (
            <p className="receipt-empty faint">Nothing opened yet.</p>
          ) : (
            <>
              <ol className="checks">
                <Check
                  tone={reading.schemaErrors.length ? 'rust' : 'ok'}
                  title={reading.schemaErrors.length ? 'Does not match the schema' : 'Matches the schema'}
                  detail={reading.schemaErrors.length ? reading.schemaErrors : ['schemas/vitna-run-receipt-v1.json']}
                />
                {receipt ? (
                  <Check
                    tone={reading.rustMissing.length ? 'rust' : 'ok'}
                    title={
                      reading.rustMissing.length
                        ? 'vitna-receipt-verify would refuse to load it'
                        : 'vitna-receipt-verify can load it'
                    }
                    detail={
                      reading.rustMissing.length
                        ? [`The Rust struct requires ${reading.rustMissing.join(', ')}, which the schema leaves optional.`]
                        : []
                    }
                  />
                ) : null}
                {receipt ? <SignatureLine check={signature} /> : null}
                {receipt ? (
                  <Check
                    tone="plain"
                    title="Event chain not recomputed"
                    detail={['A receipt carries the root of its event chain, not the events, so nothing here can rebuild it.']}
                  />
                ) : null}
                {reading.uncovered.length ? (
                  <Check
                    tone="rust"
                    title="Text no signature covers"
                    detail={[
                      `${reading.uncovered.join(', ')}: dropped before signing, so these can change without the signature noticing.`,
                    ]}
                  />
                ) : null}
              </ol>
              {receipt ? <Record receipt={receipt} /> : null}
            </>
          )}
        </section>
      </div>
    </div>
  );
}

function Check({ tone, title, detail }: { tone: 'ok' | 'rust' | 'plain'; title: string; detail: string[] }) {
  return (
    <li className={`check check-${tone}`}>
      <p className={`check-title ${tone === 'rust' ? 'tone-rust' : ''}`}>
        {tone === 'ok' ? <span className="ok-dot" aria-hidden="true" /> : null}
        {title}
      </p>
      {detail.map((line) => (
        <p key={line} className="check-detail">
          {line}
        </p>
      ))}
    </li>
  );
}

function SignatureLine({ check }: { check: SignatureCheck | null }) {
  if (!check) return <Check tone="plain" title="Checking the signature" detail={[]} />;
  switch (check.status) {
    case 'valid':
      return <Check tone="ok" title="Signature verified with this key" detail={['Ed25519 over the canonical receipt, checked in this window.']} />;
    case 'invalid':
      return <Check tone="rust" title="Signature does not match" detail={['Either a signed field changed, or this is not the key that signed it.']} />;
    case 'not_checked':
      return <Check tone="plain" title="Signature not checked" detail={['A receipt does not carry its signing key. Add the device public key above.']} />;
    case 'no_signature':
      return <Check tone="rust" title="Not signed" detail={['device_signature is empty.']} />;
    case 'placeholder':
      return <Check tone="rust" title="Placeholder signature" detail={['device_signature is all zeros, which signs nothing.']} />;
    case 'bad_signature':
    case 'bad_key':
    case 'unsupported':
      return <Check tone="rust" title="Signature could not be checked" detail={[check.detail]} />;
  }
}

function Record({ receipt }: { receipt: Receipt }) {
  const completion = completionWord(receipt.completion_state);
  const model = receipt.model_selection;
  return (
    <div className="record">
      <dl className="record-facts">
        <div>
          <dt>Completion</dt>
          <dd className={`tone-${completion.tone}`}>
            {completion.tone === 'ok' ? <span className="ok-dot" aria-hidden="true" /> : null}
            {completion.word}
          </dd>
        </div>
        <div>
          <dt>Isolation</dt>
          <dd>{isolationWord(receipt.isolation_label)}</dd>
        </div>
        <div>
          <dt>Model</dt>
          <dd>
            <span className="mono">{model.model_sku || 'not named'}</span>
            <span className="faint"> via {model.provider || 'a provider not named'}</span>
            <span className="record-sub">{model.routing_reason || 'no routing reason given'}</span>
            {model.policy_digest ? <span className="record-sub mono">policy {shortDigest(model.policy_digest, 12)}</span> : null}
          </dd>
        </div>
        <div>
          <dt>Run</dt>
          <dd className="mono">
            {receipt.run_id || 'not named'}
            <span className="record-sub">session {receipt.session_id || 'not named'}</span>
          </dd>
        </div>
        <div>
          <dt>Base commit</dt>
          <dd className="mono">{receipt.base_commit_sha || 'not stated'}</dd>
        </div>
        <div>
          <dt>Workspace</dt>
          <dd className="mono">{shortDigest(receipt.workspace_fingerprint, 16)}</dd>
        </div>
        <div>
          <dt>Event chain root</dt>
          <dd className="mono">{shortDigest(receipt.event_hash_chain_root, 16)}</dd>
        </div>
      </dl>

      <section className="record-section">
        <h2 className="eyebrow">Changes</h2>
        {receipt.changeset.files_modified.length === 0 ? (
          <p className="faint">No files recorded as changed.</p>
        ) : (
          <table className="record-table">
            <thead>
              <tr>
                <th scope="col">File</th>
                <th scope="col">Before</th>
                <th scope="col">After</th>
              </tr>
            </thead>
            <tbody>
              {receipt.changeset.files_modified.map((f) => (
                <tr key={f.path}>
                  <td className="mono">{f.path}</td>
                  <td className="mono faint">{f.preimage_hash ? shortDigest(f.preimage_hash, 10) : <span className="tone-rust">missing</span>}</td>
                  <td className="mono faint">{f.postimage_hash ? shortDigest(f.postimage_hash, 10) : <span className="tone-rust">missing</span>}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        {receipt.changeset.diff_digest ? (
          <p className="mono fainter">diff sha256 {shortDigest(receipt.changeset.diff_digest, 16)}</p>
        ) : null}
      </section>

      <section className="record-section">
        <h2 className="eyebrow">Evidence</h2>
        {receipt.evidence_items.length === 0 ? (
          <p className="faint">No evidence recorded. A completion claim with none behind it is only a claim.</p>
        ) : (
          <ul className="evidence">
            {receipt.evidence_items.map((item) => (
              <li key={item.evidence_id} className="evidence-item">
                <GradeScale grade={item.grade} />
                <div>
                  <p>{item.description}</p>
                  <p className="mono fainter">
                    {gradeWord(item.grade)} · {item.evidence_id}
                    {item.artifact_digest ? ` · artifact ${shortDigest(item.artifact_digest, 10)}` : ' · no artifact'}
                  </p>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="record-section">
        <h2 className="eyebrow">Runner statements</h2>
        {receipt.runner_execution_statements.length === 0 ? (
          <p className="faint">None recorded.</p>
        ) : (
          <ul className="statements">
            {receipt.runner_execution_statements.map((s) => {
              const shape = signatureShape(s.signature);
              return (
                <li key={s.action_id} className="mono">
                  {s.action_id} · statement {shortDigest(s.statement_digest, 10)} ·{' '}
                  {shape === 'placeholder' ? (
                    <span className="tone-rust">placeholder signature, all zeros</span>
                  ) : shape === 'empty' ? (
                    <span className="tone-rust">unsigned</span>
                  ) : (
                    <span className="faint">signature not checkable here: the protocol does not say what it signs</span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {receipt.child_receipt_roots.length > 0 ? (
        <section className="record-section">
          <h2 className="eyebrow">Child receipts</h2>
          <ul className="statements">
            {receipt.child_receipt_roots.map((root) => (
              <li key={root} className="mono">
                {shortDigest(root, 16)}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}

/** Where a grade sits on the schema's own ladder, weakest to strongest. The word is printed beside it. */
function GradeScale({ grade }: { grade: string }) {
  const index = (EVIDENCE_GRADES as readonly string[]).indexOf(grade);
  const steps = EVIDENCE_GRADES.length;
  return (
    <svg
      className="grade-scale"
      viewBox={`0 0 ${(steps - 1) * 14 + 8} 12`}
      role="img"
      aria-label={index >= 0 ? `Grade ${index + 1} of ${steps}: ${gradeWord(grade)}` : 'Grade not on the ladder'}
    >
      <line x1="4" y1="6" x2={(steps - 1) * 14 + 4} y2="6" className="grade-rule" />
      {EVIDENCE_GRADES.map((g, i) => (
        <circle key={g} cx={i * 14 + 4} cy="6" r={i === index ? 3.6 : 2} className={i === index ? 'grade-mark' : 'grade-tick'} />
      ))}
    </svg>
  );
}

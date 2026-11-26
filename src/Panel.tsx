import { toaster } from '@decky/api';
import {
  ButtonItem,
  DropdownItem,
  Field,
  PanelSection,
  PanelSectionRow,
  TextField,
  ToggleField,
} from '@decky/ui';
import { useState } from 'react';
import { BridgeState, FoundReceiver, Settings, Tone, call, copyText, useBridge } from './api';

const RECEIVER_DOWNLOAD = 'github.com/60plus/Decky-AudioBridge/releases';

const toneColour: Record<Tone, string> = {
  ok: '#5ec26a',
  warn: '#f0b429',
  error: '#ef5b5b',
  busy: '#5aa7ff',
  idle: '#8b929a',
};

const presetHelp: Record<Settings['preset'], string> = {
  low: 'Smallest delay. Best on a wired network.',
  balanced: 'Short delay with a safety margin. Good for most setups.',
  high: '24-bit sound and a larger buffer. Steadier on Wi-Fi, a little more delay.',
};

const css = `
.ab-status { padding: 10px 12px; border-radius: 6px; background: rgba(255,255,255,0.06); border-left: 4px solid var(--ab-tone); }
.ab-status-title { font-size: 17px; font-weight: 700; color: var(--ab-tone); display: flex; align-items: center; gap: 8px; }
.ab-dot { width: 10px; height: 10px; border-radius: 50%; background: var(--ab-tone); flex: none; }
.ab-status-detail { margin-top: 4px; font-size: 13px; line-height: 1.35; color: #dcdedf; }
.ab-meter { margin-top: 8px; height: 6px; border-radius: 3px; background: rgba(255,255,255,0.12); overflow: hidden; }
.ab-meter > div { height: 100%; background: var(--ab-tone); transition: width 0.25s linear; }
.ab-note { font-size: 12px; line-height: 1.4; color: #b8bcbf; padding: 2px 0; }
.ab-diag { white-space: pre-wrap; word-break: break-word; font-family: monospace; font-size: 11px; line-height: 1.35;
  max-height: 260px; overflow-y: auto; background: rgba(0,0,0,0.35); padding: 8px; border-radius: 4px; color: #dcdedf; }
`;

function meterWidth(level: number | null | undefined): number {
  if (level === null || level === undefined) return 0;
  return Math.max(0, Math.min(100, ((level + 60) / 60) * 100));
}

function StatusCard({ state, error }: { state: BridgeState | null; error: string | null }) {
  const view = error
    ? { tone: 'error' as Tone, title: 'Plugin backend not responding', detail: error }
    : state?.view ?? { tone: 'idle' as Tone, title: 'Loading', detail: '' };
  const level = state?.running ? state.status?.audio?.level_db : null;
  return (
    <div className="ab-status" style={{ ['--ab-tone' as string]: toneColour[view.tone] }}>
      <div className="ab-status-title"><span className="ab-dot" />{view.title}</div>
      {view.detail && <div className="ab-status-detail">{view.detail}</div>}
      {state?.running && (
        <div className="ab-meter" aria-label="Sound level"><div style={{ width: `${meterWidth(level)}%` }} /></div>
      )}
    </div>
  );
}

function notify(title: string, body: string) {
  toaster.toast({ title, body });
}

export function Panel() {
  const { state, setState, error, refresh } = useBridge();
  const [busy, setBusy] = useState(false);
  const [searching, setSearching] = useState(false);
  const [found, setFound] = useState<FoundReceiver[] | null>(null);
  const [draft, setDraft] = useState('');
  const [showManual, setShowManual] = useState(false);
  const [diagnostics, setDiagnostics] = useState<string | null>(null);

  const settings = state?.settings;

  async function act<T>(method: string, args?: Record<string, unknown>, onDone?: (data: T) => void) {
    setBusy(true);
    try {
      const data = await call<T>(method, args);
      onDone?.(data);
      return data;
    } catch (err) {
      notify('Audio Bridge', (err as Error).message);
      return undefined;
    } finally {
      setBusy(false);
      refresh();
    }
  }

  const applyState = (next: BridgeState) => setState(next);

  async function toggle() {
    if (!state) return;
    await act<BridgeState>(state.running ? 'stop' : 'start', undefined, applyState);
  }

  async function search() {
    setSearching(true);
    setFound(null);
    try {
      setFound(await call<FoundReceiver[]>('discover'));
    } catch (err) {
      notify('Audio Bridge', (err as Error).message);
      setFound([]);
    } finally {
      setSearching(false);
    }
  }

  async function choose(receiver: FoundReceiver) {
    await act<BridgeState>('choose_receiver', { ip: receiver.ip, port: receiver.port, name: receiver.name }, (next) => {
      applyState(next);
      setFound(null);
    });
  }

  async function useAddress() {
    await act<BridgeState>('set_address', { text: draft }, (next) => {
      applyState(next);
      setShowManual(false);
    });
  }

  async function change(changes: Partial<Settings>) {
    await act<BridgeState>('settings', { changes }, applyState);
  }

  async function loadDiagnostics() {
    if (diagnostics !== null) {
      setDiagnostics(null);
      return;
    }
    await act<string>('diagnostics', { redact: true }, setDiagnostics);
  }

  async function copyDiagnostics() {
    const text = await call<string>('diagnostics', { redact: true }).catch(() => diagnostics ?? '');
    notify('Audio Bridge', (await copyText(text)) ? 'Diagnostics copied.' : 'Could not copy. Select the text instead.');
  }

  const running = !!state?.running;
  const hasReceiver = !!settings?.receiver_host;
  const preset = state?.presets.find((p) => p.id === settings?.preset);

  return (
    <>
      <style>{css}</style>
      <PanelSection>
        <PanelSectionRow>
          <StatusCard state={state} error={error} />
        </PanelSectionRow>
        <PanelSectionRow>
          <ButtonItem layout="below" disabled={!state || busy || (!running && !hasReceiver)} onClick={toggle}>
            {running ? 'Stop streaming' : hasReceiver ? 'Start streaming' : 'Choose a receiver first'}
          </ButtonItem>
        </PanelSectionRow>
        {running && (
          <PanelSectionRow>
            <ButtonItem layout="below" disabled={busy} onClick={() => act('test_sound', undefined, () =>
              notify('Audio Bridge', 'Playing a short test sound on the receiver.'))}>
              Play a test sound on the receiver
            </ButtonItem>
          </PanelSectionRow>
        )}
      </PanelSection>

      <PanelSection title="Receiver">
        <PanelSectionRow>
          <Field label={hasReceiver ? state?.receiver_label : 'No receiver chosen'} bottomSeparator="none" />
        </PanelSectionRow>
        <PanelSectionRow>
          <ButtonItem layout="below" disabled={searching || busy} onClick={search}>
            {searching ? 'Searching…' : 'Find receivers on my network'}
          </ButtonItem>
        </PanelSectionRow>
        {found !== null && found.length === 0 && (
          <PanelSectionRow>
            <div className="ab-note">
              Nothing answered. Start Audio Bridge Receiver on your other PC, allow it in Windows Firewall when
              asked, then search again. You can also type its address below.
            </div>
          </PanelSectionRow>
        )}
        {found?.map((receiver) => (
          <PanelSectionRow key={`${receiver.ip}:${receiver.port}`}>
            <ButtonItem
              layout="below"
              disabled={busy}
              description={`${receiver.ip}${receiver.port === 6980 ? '' : `:${receiver.port}`}${receiver.ours ? '' : ` · ${receiver.application || 'VBAN receiver'}`}`}
              onClick={() => choose(receiver)}
            >
              Use {receiver.name}
            </ButtonItem>
          </PanelSectionRow>
        ))}
        <PanelSectionRow>
          <ToggleField label="Enter the address myself" checked={showManual} onChange={setShowManual} />
        </PanelSectionRow>
        {showManual && (
          <>
            <PanelSectionRow>
              <TextField
                label="IP address or computer name"
                description="Shown in the receiver window, for example 192.168.0.40"
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
              />
            </PanelSectionRow>
            <PanelSectionRow>
              <ButtonItem layout="below" disabled={busy || !draft.trim()} onClick={useAddress}>
                Use this address
              </ButtonItem>
            </PanelSectionRow>
          </>
        )}
      </PanelSection>

      <PanelSection title="Sound">
        <PanelSectionRow>
          <DropdownItem
            label="Quality"
            layout="below"
            description={settings ? presetHelp[settings.preset] : undefined}
            disabled={!state || busy}
            rgOptions={(state?.presets ?? []).map((p) => ({ data: p.id, label: p.label }))}
            selectedOption={settings?.preset}
            onChange={(option) => change({ preset: option.data })}
          />
        </PanelSectionRow>
        {running && preset && (
          <PanelSectionRow>
            <div className="ab-note">
              Changing quality restarts the stream for a moment.
            </div>
          </PanelSectionRow>
        )}
        <PanelSectionRow>
          <ToggleField
            label="Start streaming automatically"
            description="When this PC starts and a receiver is chosen."
            checked={!!settings?.auto_start}
            disabled={!state || busy}
            onChange={(value) => change({ auto_start: value })}
          />
        </PanelSectionRow>
      </PanelSection>

      <PanelSection title="Help">
        <PanelSectionRow>
          <div className="ab-note">
            The receiver for Windows is a single program you start on the other PC. Get it from {RECEIVER_DOWNLOAD}.
            Streaming keeps running when you close this menu.
          </div>
        </PanelSectionRow>
        <PanelSectionRow>
          <ButtonItem layout="below" disabled={busy && diagnostics === null} onClick={loadDiagnostics}>
            {diagnostics === null ? 'Show diagnostics' : 'Hide diagnostics'}
          </ButtonItem>
        </PanelSectionRow>
        {diagnostics !== null && (
          <>
            <PanelSectionRow>
              <div className="ab-diag">{diagnostics}</div>
            </PanelSectionRow>
            <PanelSectionRow>
              <ButtonItem layout="below" onClick={copyDiagnostics}>Copy diagnostics</ButtonItem>
            </PanelSectionRow>
          </>
        )}
        <PanelSectionRow>
          <div className="ab-note">Version {state?.version ?? '…'}</div>
        </PanelSectionRow>
      </PanelSection>
    </>
  );
}

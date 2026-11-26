import { callable } from '@decky/api';
import { useCallback, useEffect, useRef, useState } from 'react';

export type Tone = 'ok' | 'warn' | 'error' | 'busy' | 'idle';

export interface Settings {
  receiver_host: string;
  receiver_port: number;
  receiver_name: string;
  preset: 'low' | 'balanced' | 'high';
  auto_start: boolean;
  stream_name: string;
}

export interface EngineStatus {
  state: string;
  detail: string;
  stale?: boolean;
  receiver: { ip: string | null; name: string | null; application: string | null; answering: boolean; rtt_ms: number | null };
  audio: { rate: number; channels: number; format: string; level_db: number | null; packet_ms: number; capture: string };
  network: { packets: number; bytes: number; dropped: number; send_errors: number; last_error: string | null };
}

export interface BridgeState {
  version: string;
  settings: Settings;
  running: boolean;
  status: EngineStatus | null;
  view: { tone: Tone; title: string; detail: string };
  receiver_label: string;
  presets: { id: Settings['preset']; label: string; buffer_ms: number }[];
}

export interface FoundReceiver {
  ip: string;
  port: number;
  name: string;
  device: string;
  application: string;
  ours: boolean;
}

type Reply<T> = { ok: true; data: T } | { ok: false; error: string };
const request = callable<[string, Record<string, unknown>?], Reply<unknown>>('request');

export async function call<T>(method: string, args?: Record<string, unknown>): Promise<T> {
  let reply: Reply<unknown>;
  try {
    reply = await request(method, args);
  } catch (err) {
    throw new Error('The plugin backend is not responding. Try reopening the panel.');
  }
  if (!reply || typeof reply !== 'object') throw new Error('The plugin backend sent an unexpected reply.');
  if (!reply.ok) throw new Error(reply.error);
  return reply.data as T;
}

export function useBridge(pollMs = 1000) {
  const [state, setState] = useState<BridgeState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const alive = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await call<BridgeState>('state');
      if (alive.current) {
        setState(next);
        setError(null);
      }
    } catch (err) {
      if (alive.current) setError((err as Error).message);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    refresh();
    const timer = window.setInterval(refresh, pollMs);
    return () => {
      alive.current = false;
      window.clearInterval(timer);
    };
  }, [refresh, pollMs]);

  return { state, setState, error, refresh };
}

export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to the textarea method, which works in Steam's browser
  }
  const area = document.createElement('textarea');
  area.value = text;
  area.style.position = 'fixed';
  area.style.opacity = '0';
  document.body.appendChild(area);
  area.select();
  let done = false;
  try {
    done = document.execCommand('copy');
  } catch {
    done = false;
  }
  document.body.removeChild(area);
  return done;
}

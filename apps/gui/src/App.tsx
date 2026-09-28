import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import {
  absent,
  captureCommand,
  captureFields,
  captureModeText,
  captureModes,
  captureStateText,
  doctorFailed,
  guiBuild,
  notImplemented,
  stateText,
  statusFields,
  unfinishedChecks,
  type CaptureAnswer,
  type CaptureMode,
  type Diagnostics,
  type GuiState,
} from './model';
import './style.css';

/** The only commands this window may call; they are the same handlers `crates/gui` registers. */
type Command =
  | 'gui_probe'
  | 'gui_connect'
  | 'gui_doctor'
  | 'gui_capture_region'
  | 'gui_capture_window'
  | 'gui_capture_monitor'
  | 'gui_capture_desktop';

/** One bounded wait for an answer; the handler's own bound is reported in `bounded_ms`. */
const ANSWER_TIMEOUT_MS = 35_000;

/** One bounded wait for a capture answer, independent of the diagnostic attempt. */
const CAPTURE_TIMEOUT_MS = 35_000;

export default function App() {
  const [report, setReport] = useState<Diagnostics | null>(null);
  const [state, setState] = useState<GuiState>('connecting');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState('');
  const [mode, setMode] = useState<CaptureMode>('region');
  const [capture, setCapture] = useState<CaptureAnswer | null>(null);
  const inFlight = useRef(false);

  async function run(command: Command) {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setState('connecting');
    setNotice('');
    setReport(null);
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const value = await Promise.race([
        invoke<Diagnostics>(command),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new Error('timeout')), ANSWER_TIMEOUT_MS);
        }),
      ]);
      setReport(value);
      setState(value.state);
    } catch {
      // An IPC exception may carry a local path or a native detail, so only a stable
      // marker is shown: the honest answer is that no verified state was received.
      setState('host_not_running');
      setNotice('GUI_TRANSPORT_UNAVAILABLE');
    } finally {
      clearTimeout(timer);
      inFlight.current = false;
      setBusy(false);
    }
  }

  useEffect(() => {
    void run('gui_probe');
  }, []);

  async function openReport() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    try {
      const result = await invoke<{ opened: boolean; safe_code: string | null }>('gui_open_report');
      setNotice(result.opened ? 'Открыта папка диагностических отчётов.' : (result.safe_code ?? 'REPORT_UNAVAILABLE'));
    } catch {
      setNotice('REPORT_UNAVAILABLE');
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }

  /**
   * The capture action: the *same* host command the CLI uses. A region capture waits for
   * the user, so the wait is bounded separately from the diagnostic attempt.
   */
  async function startCapture() {
    if (inFlight.current) return;
    inFlight.current = true;
    setBusy(true);
    setCapture(null);
    setNotice('');
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      const answer = await Promise.race([
        // Argument free: the mode selects the handler, nothing is sent as a parameter.
        invoke<CaptureAnswer>(captureCommand[mode]),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new Error('timeout')), CAPTURE_TIMEOUT_MS);
        }),
      ]);
      setCapture(answer);
      setNotice(captureStateText[answer.state]);
    } catch {
      setNotice('CAPTURE_TRANSPORT_UNAVAILABLE');
    } finally {
      clearTimeout(timer);
      inFlight.current = false;
      setBusy(false);
    }
  }

  const unfinished = unfinishedChecks(report);
  const absentCapabilities = notImplemented(report);

  return (
    <main>
      <h1>ScreenMemory</h1>
      <p>
        Диагностика основы · S01 · Test preview · сборка {guiBuild.version}
      </p>
      <section aria-live="polite" aria-busy={busy}>
        <h2>Соединение</h2>
        <p>{stateText[state]}</p>
        {report?.safe_code && <p role="alert">Безопасный код: {report.safe_code}</p>}
        {report?.retryable && <p>Повтор разрешён.</p>}
        <p>Ограничение одного обращения: {report ? `${report.bounded_ms} ms` : absent}.</p>
        {notice && <p role="status">{notice}</p>}
      </section>
      <nav aria-label="Диагностика">
        <button disabled={busy} onClick={() => void run('gui_connect')}>
          Подключиться
        </button>
        <button disabled={busy} onClick={() => void run('gui_doctor')}>
          Проверить основу
        </button>
        <button disabled={busy || !report?.report_available} onClick={() => void openReport()}>
          Открыть диагностический отчёт
        </button>
      </nav>
      <section aria-live="polite">
        <h2>Захват экрана</h2>
        <label>
          Режим{' '}
          <select
            value={mode}
            disabled={busy}
            onChange={(event) => setMode(event.target.value as CaptureMode)}
          >
            {captureModes.map((name) => (
              <option key={name} value={name}>
                {captureModeText[name]}
              </option>
            ))}
          </select>
        </label>
        <button disabled={busy} onClick={() => void startCapture()}>
          Сделать снимок
        </button>
        <dl>
          {captureFields(capture).map(([key, value]) => (
            <div key={key}>
              <dt>{key}</dt>
              <dd>{value}</dd>
            </div>
          ))}
        </dl>
      </section>
      <dl>
        {statusFields(report).map(([key, value]) => (
          <div key={key}>
            <dt>{key}</dt>
            <dd>{value}</dd>
          </div>
        ))}
      </dl>
      {report?.doctor && (
        <section>
          <h2>Проверка основы</h2>
          <p>
            {doctorFailed(report)
              ? 'Обнаружен провал обязательной проверки.'
              : 'Обязательные проверки не провалены; неподдержанные проверки не считаются выполненными.'}
          </p>
          <ul>
            {report.doctor.checks.map((check) => (
              <li key={check.check_id}>
                {check.check_id}: {check.outcome}
                {check.safe_code ? ` (${check.safe_code})` : ''}
              </li>
            ))}
          </ul>
          {unfinished.length > 0 && (
            <p>Не подтверждено на этом этапе: {unfinished.length} проверок (NOT_RUN/BLOCKED/UNSUPPORTED).</p>
          )}
        </section>
      )}
      <section>
        <h2>Вне текущего этапа</h2>
        <p>Галерея, поиск, счётчики изображений, drop zone, настройки моделей и AI bridge — не реализовано на этом этапе.</p>
        {absentCapabilities.length > 0 && <p>Отсутствующие возможности по данным host: {absentCapabilities.join(', ')}.</p>}
      </section>
      <p>Закрытие окна не останавливает host. Вызовов моделей и загрузки весов нет.</p>
    </main>
  );
}

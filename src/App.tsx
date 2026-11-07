import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Activity, ArrowDownToLine, ArrowUpFromLine, Check, CheckCheck, ChevronRight, Circle, Database, Gamepad2, History, LockKeyhole, Pause, Play, RefreshCw, Search, ShieldCheck, Square, Terminal, X } from 'lucide-react';
import type { SpooferConfig, SpooferStatus, SpooferTickEvent, SpooferLogEvent, SpooferStateResponse, SpoofableGameItem } from './types/spoofer';

type Tab = 'activity' | 'games' | 'history' | 'security';
const desktop = '__TAURI_INTERNALS__' in window;
const defaults: SpooferConfig = { batch_size: 1, hold_duration_secs: 60, limit: null, start_offset: 0, skip_previously_spoofed: true, spoof_mode: 'process_only', beta_enabled: false, cooldown_enabled: true, cooldown_every_n_batches: 10, cooldown_seconds: 30, jitter_enabled: false, farm_game_ids: [], farm_duration_secs: 3600 };
const initialTick: SpooferTickEvent = { current_index: 0, total_games: 0, active_batch: [], elapsed_secs: 0, estimated_remaining_secs: 0, games_per_minute: 0 };
const statusNames: Record<SpooferStatus, string> = { idle: 'Ожидание', running: 'Активна', paused: 'На паузе', finished: 'Завершена', error: 'Ошибка' };
const duration = (v: number) => `${String(Math.floor(v / 3600)).padStart(2, '0')}:${String(Math.floor(v % 3600 / 60)).padStart(2, '0')}:${String(v % 60).padStart(2, '0')}`;

export default function App() {
  const [tab, setTab] = useState<Tab>('activity');
  const [status, setStatus] = useState<SpooferStatus>('idle');
  const [tick, setTick] = useState(initialTick);
  const [config, setConfig] = useState(defaults);
  const [games, setGames] = useState<SpoofableGameItem[]>([]);
  const [logs, setLogs] = useState<Omit<SpooferLogEvent, 'id'>[]>([]);
  const [history, setHistory] = useState<string[]>([]);
  const [discord, setDiscord] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [query, setQuery] = useState('');
  const [farm, setFarm] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const [filter, setFilter] = useState<'process' | 'rpc'>('process');
  const file = useRef<HTMLInputElement>(null);
  const active = status === 'running' || status === 'paused';
  const processGames = games.filter(g => g.method === 'process');
  const available = games.filter(g => config.spoof_mode === 'process_only' ? g.method === 'process' : g.method === 'rpc');
  const selected = config.farm_game_ids ?? [];
  const filtered = useMemo(() => games.filter(g => g.method === filter && g.name.toLocaleLowerCase().includes(query.toLocaleLowerCase())), [games, filter, query]);
  const addLog = (message: string, level: 'info' | 'error' = 'info') => setLogs(old => [...old.slice(-199), { message, level, timestamp: new Date().toLocaleTimeString('ru-RU') }]);
  async function refreshHistory() { setHistory(JSON.parse(await invoke<string>('export_history'))); }
  async function action(fn: () => Promise<void>) {
    setBusy(true); setError('');
    try { await fn(); } catch (e) { setError(String(e)); addLog(String(e), 'error'); } finally { setBusy(false); }
  }
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const cleanup: UnlistenFn[] = [];
    async function setup() {
      const register = async <T,>(event: string, cb: (payload: T) => void) => {
        const off = await listen<T>(event, e => { if (!disposed) cb(e.payload); });
        if (disposed) off(); else cleanup.push(off);
      };
      try {
        await register<SpooferTickEvent>('spoofer_tick', setTick);
        await register<Omit<SpooferLogEvent, 'id'>>('spoofer_log', v => setLogs(old => [...old.slice(-199), v]));
        await register<{status: SpooferStatus; message?: string}>('spoofer_status', v => {
          setStatus(v.status); if (v.message) setError(v.message);
          if (v.status === 'finished' || v.status === 'idle' || v.status === 'error') {
            setTick(t => ({...t, active_batch: []}));
            invoke<string>('export_history').then(v => { if (!disposed) setHistory(JSON.parse(v)); }).catch(e => { if (!disposed) setError(String(e)); });
          }
        });
        const state = await invoke<SpooferStateResponse>('get_spoofer_state');
        if (!disposed) setStatus(state.status);
        const ids = await invoke<string>('export_history');
        if (!disposed) setHistory(JSON.parse(ids));
        const running = await invoke<boolean>('check_discord_status');
        if (!disposed) setDiscord(running);
      } catch (e) { if (!disposed) setError(String(e)); }
    }
    void setup();
    const timer = window.setInterval(() => { invoke<boolean>('check_discord_status').then(v => { if (!disposed) setDiscord(v); }).catch(() => { if (!disposed) setDiscord(false); }); }, 5000);
    return () => { disposed = true; cleanup.forEach(off => off()); clearInterval(timer); };
  }, []);
  async function load(force = false) {
    await action(async () => {
      const res = await invoke<{from_cache: boolean}>('fetch_games', {force});
      setGames(await invoke<SpoofableGameItem[]>('get_games_list'));
      addLog(res.from_cache ? 'База загружена из локального кеша.' : 'Каталог получен с CDN Discord по HTTPS.');
    });
  }
  async function start() {
    if (farm && selected.length === 0) { setError('Выберите игры на вкладке «Библиотека».'); return; }
    await action(async () => {
      setTick(initialTick);
      await invoke('start_spoofer', {config: {...config, farm_game_ids: farm ? selected : []}});
    });
  }
  function choose(id: string) {
    setConfig(c => { const ids = c.farm_game_ids ?? []; return {...c, farm_game_ids: ids.includes(id) ? ids.filter(v => v !== id) : [...ids, id].slice(0, 4)}; });
  }
  async function exportHistory() {
    await action(async () => {
      const json = await invoke<string>('export_history');
      const url = URL.createObjectURL(new Blob([json], {type: 'application/json'}));
      const a = document.createElement('a'); a.href = url; a.download = 'mimic-linux-history.json'; a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    });
  }
  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><div className="brand-mark">m<span>·</span></div><div>mimic<span>LINUX EDITION</span></div></div>
      <div className="workspace-label">РАБОЧЕЕ ПРОСТРАНСТВО</div>
      <nav>{([
        ['activity', Activity, 'Активность'], ['games', Gamepad2, 'Библиотека'], ['history', History, 'История'], ['security', ShieldCheck, 'Безопасность'],
      ] as const).map(([id, Icon, name]) => <button key={id} onClick={() => setTab(id)} className={`nav-item ${tab === id ? 'selected' : ''}`}><Icon size={18}/>{name}{id === 'games' && <span className="nav-count">{processGames.length}</span>}</button>)}</nav>
      <div className="sidebar-bottom"><div className="local-note"><LockKeyhole size={16}/><div>Локальное приложение<small>Без токена аккаунта</small></div></div><div className="build-label">mimic linux <span>v0.2.0</span></div></div>
    </aside>
    <main>
      <header className="topbar"><div className="breadcrumb">Рабочее пространство <ChevronRight size={14}/><span>{{activity: 'Активность', games: 'Библиотека', history: 'История', security: 'Безопасность'}[tab]}</span></div><div className="linux-tag"><Terminal size={13}/> LINUX</div></header>
      <div className="content">
        <div className="page-heading"><div><div className="eyebrow">MIMIC / LOCAL ACTIVITY</div><h1>{{activity: 'Твоя активность. Твой контроль.', games: 'Библиотека игр', history: 'История сессий', security: 'Всё под контролем'}[tab]}</h1><p>{{activity: 'Управляй игровой активностью Discord с рабочего стола Linux.', games: 'Только реальные записи из базы Discord. Доступны Linux- и Windows-имена процессов без RPC.', history: 'Здесь хранятся игры, для которых локально выдержан заданный интервал.', security: 'Прозрачные ограничения и минимум доступа к системе.'}[tab]}</p></div><div className={`connection ${discord ? 'online' : ''}`}><i/>{!desktop ? 'Предпросмотр интерфейса' : discord ? 'Discord запущен' : 'Discord не запущен'}</div></div>
        {!desktop && <div className="notice">Это браузерный предпросмотр. Загрузка базы и запуск доступны в настольном приложении.</div>}
        {error && <div className="error-box" role="alert"><span>{error}</span><button aria-label="Закрыть ошибку" onClick={() => setError('')}><X size={16}/></button></div>}
        {tab === 'activity' && <>
          <section className="stats-grid"><div className="stat-card"><div className="stat-label">БЕЗ RPC · ПРОЦЕССЫ <Gamepad2 size={16}/></div><strong>{games.length ? processGames.length.toString().padStart(2, '0') : '—'}</strong><small>{games.length ? 'кандидатов на распознавание' : 'загрузите базу игр'}</small></div><div className="stat-card"><div className="stat-label">ТЕКУЩАЯ СЕССИЯ <Activity size={16}/></div><strong className="stat-time">{duration(tick.elapsed_secs)}</strong><small><i className={`status-dot ${active ? 'green' : ''}`}/>{statusNames[status]}</small></div><div className="stat-card"><div className="stat-label">ЛОКАЛЬНАЯ ИСТОРИЯ <CheckCheck size={16}/></div><strong>{history.length.toString().padStart(2, '0')}</strong><small>не подтверждает зачёт Discord</small></div></section>
          <div className="notice">Режим процессов работает без RPC и Wine, включая игры с Windows-именами файлов. Распознавание и зачёт времени зависят от версии Discord.</div>
          <div className="dashboard-grid"><section className="panel session-panel"><div className="panel-heading"><div><h2>Настройка сессии</h2><p>Выбери режим и темп активности</p></div><span className="subtle-badge">01 / СЕССИЯ</span></div>
            <div className="segmented"><button disabled={active} className={!farm ? 'on' : ''} onClick={() => setFarm(false)}><RefreshCw size={15}/> Смена игр</button><button disabled={active} className={farm ? 'on' : ''} onClick={() => setFarm(true)}><Gamepad2 size={15}/> Фиксированный набор</button></div>
            <div className="field-grid"><label>Одновременно<span className="input-wrap"><select aria-label="Одновременно" disabled={active} value={config.batch_size} onChange={e => setConfig({...config, batch_size: Number(e.target.value)})}>{[1,2,3,4].map(v => <option key={v} value={v}>{v} {v === 1 ? 'процесс' : 'процесса'}</option>)}</select></span></label><label>{farm ? 'Длительность сессии' : 'Удержание игры'}<span className="input-wrap"><input aria-label={farm ? 'Длительность сессии' : 'Удержание игры'} disabled={active} type="number" min="15" max={farm ? 28800 : 3600} value={farm ? config.farm_duration_secs ?? 3600 : config.hold_duration_secs} onChange={e => setConfig({...config, [farm ? 'farm_duration_secs' : 'hold_duration_secs']: Number(e.target.value)})}/><span>сек</span></span></label></div>
            {farm ? <button disabled={active} className="selection-link" onClick={() => setTab('games')}>Выбрано игр: {selected.length} / 4 <ChevronRight size={16}/></button> : <label className="check-row"><input type="checkbox" disabled={active} checked={config.skip_previously_spoofed} onChange={e => setConfig({...config, skip_previously_spoofed: e.target.checked})}/><div>Пропускать игры из истории<small>Продолжить с места предыдущей сессии</small></div></label>}
            <div className="session-limit"><ShieldCheck size={17}/><span>До 4 активностей · автостоп через 8 часов</span></div>
            <div className="controls">{active ? <><button className="primary" disabled={busy} onClick={() => void action(async () => { await invoke(status === 'paused' ? 'resume_spoofer' : 'pause_spoofer'); })}>{status === 'paused' ? <Play size={17}/> : <Pause size={17}/>} {status === 'paused' ? 'Продолжить' : 'Пауза'}</button><button className="stop" disabled={busy} onClick={() => void action(async () => { await invoke('stop_spoofer'); })}><Square size={15}/> Остановить</button></> : <button className="primary" disabled={!desktop || busy || !games.length || !discord} onClick={() => void start()}><Play size={17}/>{busy ? 'Подготовка…' : 'Запустить сессию'}</button>}</div>
            <p className="control-footnote">{active ? 'Пауза останавливает таймер и смену игр. Текущая активность остаётся до остановки.' : 'Разреши отображение игр в настройках активности Discord.'}</p>
          </section>
          <section className="panel monitor-panel"><div className="panel-heading"><div><h2>Сейчас в активности</h2><p>Процессы, запущенные этой сессией</p></div><span className={`live-label ${active ? 'live' : ''}`}><i/>{active ? 'LIVE' : 'ОЖИДАНИЕ'}</span></div>
            {tick.active_batch.length ? <div className="active-list">{tick.active_batch.map(g => <div className="active-game" key={g.id}><div className="game-icon"><Gamepad2 size={22}/></div><div><b>{g.name}</b><small>{g.method === 'process' ? `PID ${g.pid}` : 'Discord IPC · подтверждено'}</small></div><span className="active-indicator"/></div>)}<div className="progress-line"><span>Осталось в интервале</span><b>{duration(tick.estimated_remaining_secs)}</b></div></div> : <div className="empty-state"><div className="empty-orbit"><Gamepad2 size={32}/><span/></div><h3>Готовы, когда готов ты</h3><p>Загрузи базу, настрой сессию<br/>и запусти первую активность.</p><button className="text-button" disabled={!desktop || busy} onClick={() => void load()}><Database size={14}/> {busy ? 'Загружаем…' : 'Загрузить базу игр'} <ChevronRight size={13}/></button></div>}
            <div className="monitor-footer"><span><LockKeyhole size={13}/> Без изменения клиента Discord</span><span>{config.beta_enabled ? 'RPC · эксперимент' : 'Процессы · без RPC'}</span></div>
          </section></div>
          <section className="panel log-panel"><div className="log-header"><h2><Terminal size={16}/> Журнал событий <span>{logs.length}</span></h2><button className="text-button" disabled={!logs.length} onClick={() => setLogs([])}>Очистить</button></div><div className="logs" role="log" aria-live="polite">{logs.length ? logs.slice(-60).map((l,i) => <div className={`log-row ${l.level}`} key={i}><time>{l.timestamp}</time><span>{l.level.toUpperCase()}</span><p>{l.message}</p></div>) : <div className="log-placeholder"><Circle size={8}/><span>События сессии появятся здесь. Приложение ничего не запускает автоматически.</span></div>}</div></section>
        </>}
        {tab === 'games' && <section className="panel library-panel"><div className="panel-heading"><div><h2>Игры Discord <span className="subtle-badge">{games.length.toLocaleString('ru-RU')}</span></h2><p>Выбери до четырёх игр для фиксированного набора.</p></div><div className="button-row"><button className="secondary" disabled={!desktop || busy || active} onClick={() => void load()}><Database size={15}/>{busy ? 'Загрузка…' : 'Загрузить'}</button><button className="icon-button" aria-label="Обновить базу из Discord" disabled={!desktop || busy || active} onClick={() => void load(true)}><RefreshCw size={16}/></button></div></div><div className="library-toolbar"><div className="segmented compact"><button className={filter === 'process' ? 'on' : ''} onClick={() => setFilter('process')}>Процессы · {processGames.length}</button>{config.beta_enabled && <button className={filter === 'rpc' ? 'on' : ''} onClick={() => setFilter('rpc')}>RPC · {games.length - processGames.length}</button>}</div><label className="search"><Search size={16}/><input aria-label="Поиск игр" placeholder="Найти игру…" value={query} onChange={e => setQuery(e.target.value)}/></label></div><div className="game-table"><div className="table-label"><span>ИГРА / ПРИЛОЖЕНИЕ</span><span>СПОСОБ</span><span>В НАБОР</span></div>{filtered.slice(0,200).map(g => <div className="game-row" key={g.id}><div className="game-title"><div className="game-icon"><Gamepad2 size={19}/></div><div><b>{g.name}</b><small>{g.rel_path ?? 'Экспериментальная активность по IPC'}</small></div></div><span className="method-badge">{g.method === 'process' ? 'Процесс' : 'RPC'}</span><button className={`choose ${selected.includes(g.id) ? 'chosen' : ''}`} disabled={active || (g.method === 'rpc' && config.spoof_mode !== 'rpc_only') || (g.method === 'process' && config.spoof_mode !== 'process_only') || (!selected.includes(g.id) && selected.length >= 4)} aria-label={`Выбрать ${g.name}`} aria-pressed={selected.includes(g.id)} onClick={() => choose(g.id)}>{selected.includes(g.id) ? <Check size={16}/> : '+'}</button></div>)}{filtered.length === 0 && <div className="table-empty"><Database size={28}/><h3>{games.length ? 'Ничего не найдено' : 'Твоя библиотека начинается здесь'}</h3><p>{games.length ? 'Измени запрос или способ активности.' : 'Нажми «Загрузить», чтобы получить доступные игры.'}</p></div>}{filtered.length > 200 && <p className="control-footnote">Показаны первые 200 игр. Уточни поиск.</p>}</div><div className="library-footer"><span>Выбрано {selected.length} из 4 · режим {config.spoof_mode === 'process_only' ? 'Процессы' : 'RPC'}</span><button className="text-button" onClick={() => { setFarm(true); setTab('activity'); }}>Перейти к сессии <ChevronRight size={15}/></button></div></section>}
        {tab === 'history' && <section className="panel library-panel"><div className="panel-heading"><div><h2>Локальная история <span className="subtle-badge">{history.length}</span></h2><p>Только полностью выдержанные интервалы смены игр. Фиксированный набор сюда не добавляется.</p></div></div><div className="history-actions"><button className="secondary" disabled={!desktop || busy || !history.length} onClick={() => void exportHistory()}><ArrowDownToLine size={16}/> Экспорт JSON</button><button className="secondary" disabled={!desktop || busy || active} onClick={() => file.current?.click()}><ArrowUpFromLine size={16}/> Импорт JSON</button><button className="danger-link" disabled={!desktop || busy || active || !history.length} onClick={() => setConfirmClear(true)}>Очистить историю</button></div><input ref={file} type="file" accept="application/json,.json" hidden onChange={e => { const f = e.target.files?.[0]; e.target.value = ''; if (!f) return; void action(async () => { if (f.size > 4 * 1024 * 1024) throw new Error('Максимальный размер файла — 4 МБ'); const n = await invoke<number>('import_history', {jsonData: await f.text()}); await refreshHistory(); addLog(`Импортировано новых записей: ${n}`); }); }}/>{confirmClear && <div className="confirm-box"><p>Удалить локальную историю? Прогресс в Discord это не изменит.</p><button className="stop" disabled={busy} onClick={() => void action(async () => { await invoke('clear_history'); await refreshHistory(); setConfirmClear(false); })}>Удалить историю</button><button className="secondary" onClick={() => setConfirmClear(false)}>Отмена</button></div>}{history.length ? <div className="history-list">{history.slice(0,200).map(id => <div key={id}><CheckCheck size={17}/><span>{games.find(g => g.id === id)?.name ?? `Приложение ${id}`}</span><code>{id}</code></div>)}</div> : <div className="table-empty"><History size={30}/><h3>Пока чистый лист</h3><p>Завершённые интервалы появятся здесь после сессии.</p></div>}</section>}
        {tab === 'security' && <><section className="panel security-panel"><div className="security-hero"><div className="shield-icon"><ShieldCheck size={34}/></div><div><h2>Меньше доступа. Больше контроля.</h2><p>Приложение работает от обычного пользователя и не читает данные аккаунта.</p></div></div><div className="security-grid">{[['Без токенов и паролей','Не запрашиваем учётные данные. Подключаемся к локальному Discord.'],['Изолированные процессы','Запускаем только собственный встроенный помощник, без команд из базы игр.'],['Закрытые локальные данные','История и кеш хранятся в папке пользователя с ограниченными правами.'],['Предсказуемая остановка','Кнопка «Стоп» и закрытие приложения завершают наши процессы.']].map(([title, text]) => <div className="security-feature" key={title}><Check size={17}/><div><h3>{title}</h3><p>{text}</p></div></div>)}</div></section><section className="panel settings-panel"><div><h2>Экспериментальный RPC</h2><p>Для игр без Linux-файла. Подтверждение команды IPC не означает зачёт значков или времени.<br/>Нельзя гарантировать отсутствие санкций Discord за имитацию активности.</p></div><label className="switch"><input aria-label="Экспериментальный RPC" disabled={active} type="checkbox" checked={config.beta_enabled} onChange={e => { setConfig({...config, beta_enabled: e.target.checked, spoof_mode: e.target.checked ? 'rpc_only' : 'process_only', farm_game_ids: []}); setFilter(e.target.checked ? 'rpc' : 'process'); }}/><span/></label></section><div className="notice">В RPC-режиме Discord сам выбирает отображаемую активность; несколько соединений не гарантируют одновременный показ нескольких игр. Сетевой запрос к базе выполняется только по нажатию «Загрузить» или «Обновить».</div><section className="panel security-panel"><h2>О проекте</h2><p className="about-text">Mimic Linux — отдельная Linux-версия на основе Timon2306/mimic-discord (MIT). Исходники, изменения и ограничения описаны в README и SECURITY.md. Это стороннее приложение без аффилированности с Discord.</p></section></>}
        <footer className="page-footer"><span><ShieldCheck size={13}/> Локально. Без токена. Под твоим контролем.</span><span>Зачёт активности определяет Discord</span></footer>
        {tab === 'activity' && available.length === 0 && games.length > 0 && <div className="notice">Для выбранного режима нет игр. Проверь базу или выбери другой режим в разделе «Безопасность».</div>}
      </div>
    </main>
  </div>;
}

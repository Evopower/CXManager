import {
  AlertCircle,
  Check,
  ChevronRight,
  FolderOpen,
  Gauge,
  Loader2,
  Plus,
  RefreshCw,
  Save,
  ShieldCheck,
  Terminal,
  Trash2,
  Wrench
} from "lucide-react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import appIcon from "./assets/app-icon.svg";
import { invokeCommand } from "./api";
import { AppSettings, AppState, CodexStatus, RepairResult, RuntimeStatus, SaveResult, TargetShell, ToolProgressEvent, UpdateResult } from "./types";

const targetShellLabels: Record<TargetShell, string> = {
  auto: "自动",
  pwsh: "PowerShell 7",
  powershell: "Windows PowerShell 5.1"
};

function statusClass(ok: boolean) {
  return ok ? "statusPill ok" : "statusPill warn";
}

function isTauriRuntime() {
  return "__TAURI_INTERNALS__" in window;
}

function formatVersion(status: CodexStatus) {
  if (!status.localVersion) return "未检测到";
  if (!status.latestVersion) return status.localVersion;
  return `${status.localVersion} / ${status.latestVersion}`;
}

function formatToolProgress(event: ToolProgressEvent) {
  const prefix = event.done
    ? event.success
      ? "完成"
      : "失败"
    : event.stream === "stderr"
      ? "ERR"
      : event.stream === "system"
        ? "INFO"
        : "OUT";
  const message = event.message.replace(/\r\n/g, "\n").replace(/\r/g, "\n").trimEnd();
  const lines = message.split("\n").filter((line) => line.trim().length > 0);
  return (lines.length ? lines : [message]).map((line) => `[${prefix}] ${line}`);
}

export default function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("正在加载 CX-Manager");
  const [error, setError] = useState<string | null>(null);
  const [toolRunning, setToolRunning] = useState<string | null>(null);
  const [toolLogs, setToolLogs] = useState<string[]>([]);

  useEffect(() => {
    void loadState();
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let active = true;
    let unlisten: UnlistenFn | null = null;

    listen<ToolProgressEvent>("tool-progress", (event) => {
      const payload = event.payload;
      setToolLogs((logs) => [...logs, ...formatToolProgress(payload)].slice(-240));
      setToolRunning(payload.done ? null : payload.action);
      if (payload.done && payload.success === false) {
        setStatus(`${payload.action}失败`);
      }
    })
      .then((handler) => {
        if (active) {
          unlisten = handler;
        } else {
          handler();
        }
      })
      .catch((err) => {
        setToolLogs((logs) => [...logs, `[ERR] 无法监听安装日志: ${String(err)}`].slice(-240));
      });

    return () => {
      active = false;
      if (unlisten) unlisten();
    };
  }, []);

  async function loadState() {
    setLoading(true);
    setError(null);
    try {
      const loaded = await invokeCommand<AppState>("load_app_state");
      setState(loaded);
      setStatus(loaded.profileStatus.message);
    } catch (err) {
      setError(String(err));
      setStatus("加载失败");
    } finally {
      setLoading(false);
    }
  }

  function updateSettings(patch: Partial<AppSettings>) {
    if (!state) return;
    setState({
      ...state,
      settings: {
        ...state.settings,
        ...patch
      }
    });
  }

  function addProjectRootPath(root: string) {
    if (!state) return;
    const selectedRoot = root.trim();
    if (!selectedRoot) return;
    if (state.settings.projectRoots.some((item) => item.toLowerCase() === selectedRoot.toLowerCase())) {
      setError("这个项目根目录已经存在。");
      return;
    }
    updateSettings({ projectRoots: [...state.settings.projectRoots, selectedRoot] });
    setStatus(`已添加项目根目录: ${selectedRoot}`);
    setError(null);
  }

  async function chooseProjectRoot() {
    setError(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: "选择项目根目录"
      });
      if (typeof selected === "string") {
        addProjectRootPath(selected);
      }
    } catch (err) {
      setError(String(err));
      setStatus("选择项目根目录失败");
    }
  }

  function deleteProjectRoot(root: string) {
    if (!state) return;
    updateSettings({ projectRoots: state.settings.projectRoots.filter((item) => item !== root) });
  }

  async function save() {
    if (!state) return;
    setBusy(true);
    setError(null);
    try {
      const result = await invokeCommand<SaveResult>("save_app_state", { settings: state.settings });
      setState({
        ...state,
        shellStatus: result.shellStatus,
        profileStatus: result.profileStatus
      });
      setStatus(result.message);
    } catch (err) {
      setError(String(err));
      setStatus("保存失败");
    } finally {
      setBusy(false);
    }
  }

  async function repairProfile() {
    if (!state) return;
    setBusy(true);
    setError(null);
    try {
      const result = await invokeCommand<RepairResult>("repair_profile", { settings: state.settings });
      setState({
        ...state,
        profileStatus: result.profileStatus
      });
      setStatus(result.message);
    } catch (err) {
      setError(String(err));
      setStatus("修复失败");
    } finally {
      setBusy(false);
    }
  }

  function applyToolResult(result: UpdateResult) {
    if (!state) return;
    setState({
      ...state,
      toolchainStatus: result.toolchainStatus,
      codexStatus: result.codexStatus
    });
    setStatus(result.message);
  }

  function beginToolAction(action: string) {
    setToolRunning(action);
    setToolLogs([`[INFO] ${action}`]);
  }

  function appendToolError(err: unknown) {
    setToolRunning(null);
    setToolLogs((logs) => [...logs, `[失败] ${String(err)}`].slice(-240));
  }

  async function refreshRuntime() {
    if (!state) return;
    setBusy(true);
    setError(null);
    try {
      const runtimeStatus = await invokeCommand<RuntimeStatus>("refresh_runtime_status", {
        settings: state.settings
      });
      setState({
        ...state,
        toolchainStatus: runtimeStatus.toolchainStatus,
        codexStatus: runtimeStatus.codexStatus
      });
      setStatus(runtimeStatus.codexStatus.message);
    } catch (err) {
      setError(String(err));
      setStatus("刷新运行环境状态失败");
    } finally {
      setBusy(false);
    }
  }

  async function updateCodex() {
    if (!state) return;
    setBusy(true);
    setError(null);
    beginToolAction("更新 Codex");
    try {
      const result = await invokeCommand<UpdateResult>("update_codex", {
        settings: state.settings
      });
      applyToolResult(result);
    } catch (err) {
      setError(String(err));
      setStatus("Codex 更新失败");
      appendToolError(err);
    } finally {
      setBusy(false);
    }
  }

  async function installCodex() {
    if (!state) return;
    setBusy(true);
    setError(null);
    beginToolAction("安装 Codex CLI");
    try {
      const result = await invokeCommand<UpdateResult>("install_codex", {
        settings: state.settings
      });
      applyToolResult(result);
    } catch (err) {
      setError(String(err));
      setStatus("Codex 安装失败");
      appendToolError(err);
    } finally {
      setBusy(false);
    }
  }

  async function installNodejs() {
    if (!state) return;
    setBusy(true);
    setError(null);
    beginToolAction("安装 Node.js LTS / npm");
    try {
      const result = await invokeCommand<UpdateResult>("install_nodejs", {
        settings: state.settings
      });
      applyToolResult(result);
    } catch (err) {
      setError(String(err));
      setStatus("Node.js / npm 安装失败");
      appendToolError(err);
    } finally {
      setBusy(false);
    }
  }

  const helperSummary = useMemo(() => {
    if (!state) return "";
    const missing = state.profileStatus.missingHelpers;
    return missing.length ? missing.join(", ") : "完整";
  }, [state]);

  if (loading) {
    return (
      <main className="boot">
        <Loader2 className="spin" size={28} />
        <span>加载 CX-Manager</span>
      </main>
    );
  }

  if (!state) {
    return (
      <main className="boot">
        <AlertCircle size={28} />
        <span>{error ?? "无法加载应用状态"}</span>
      </main>
    );
  }

  return (
    <div className="app">
      <aside className="rail">
        <div className="brand">
          <img src={appIcon} alt="" />
          <div>
            <h1>CX-Manager</h1>
            <p>{targetShellLabels[state.shellStatus.targetShell]}</p>
          </div>
        </div>

        <div className="railBlock">
          <p className="eyebrow">Target</p>
          <label>
            <span>Shell</span>
            <select
              value={state.settings.targetShell}
              onChange={(event) => updateSettings({ targetShell: event.target.value as TargetShell })}
            >
              <option value="auto">自动</option>
              <option value="pwsh">PowerShell 7</option>
              <option value="powershell">Windows PowerShell 5.1</option>
            </select>
          </label>
          <div className="metaLine">
            <Terminal size={15} />
            <span>{state.shellStatus.command}</span>
          </div>
          <div className="metaLine">
            <Gauge size={15} />
            <span>{state.shellStatus.version ?? "unknown"}</span>
          </div>
        </div>

        <div className="railActions">
          <button onClick={() => void loadState()} disabled={busy}>
            <RefreshCw size={16} />
            重新加载
          </button>
          <button onClick={repairProfile} disabled={busy}>
            <Wrench size={16} />
            修复
          </button>
          <button className="primary" onClick={save} disabled={busy}>
            {busy ? <Loader2 className="spin" size={16} /> : <Save size={16} />}
            保存
          </button>
        </div>
      </aside>

      <main className="content">
        <header className="topbar">
          <div>
            <p className="eyebrow">PowerShell Profile</p>
            <h2>{state.profileStatus.profilePath}</h2>
          </div>
          <div className={statusClass(state.profileStatus.isComplete)}>
            {state.profileStatus.isComplete ? <Check size={15} /> : <AlertCircle size={15} />}
            {state.profileStatus.isComplete ? "Ready" : "Needs repair"}
          </div>
        </header>

        {error && (
          <div className="alert">
            <AlertCircle size={16} />
            <span>{error}</span>
          </div>
        )}

        <section className="grid">
          <section className="panel overviewPanel">
            <div className="panelHeader">
              <div>
                <p className="eyebrow">Codex</p>
                <h3>{formatVersion(state.codexStatus)}</h3>
              </div>
              <button onClick={refreshRuntime} disabled={busy} title="刷新运行环境状态">
                <RefreshCw size={16} />
              </button>
            </div>

            <div className="statusRows">
              <div>
                <span>Node.js</span>
                <strong>{state.toolchainStatus.node.version ?? "not found"}</strong>
              </div>
              <div>
                <span>npm</span>
                <strong>{state.toolchainStatus.npm.version ?? "not found"}</strong>
              </div>
              <div>
                <span>本地版本</span>
                <strong>{state.codexStatus.localVersion ?? "unknown"}</strong>
              </div>
              <div>
                <span>最新版本</span>
                <strong>{state.codexStatus.latestVersion ?? "unknown"}</strong>
              </div>
              <div>
                <span>可执行路径</span>
                <strong title={state.codexStatus.executablePaths[0] ?? ""}>
                  {state.codexStatus.executablePaths[0] ?? "not found"}
                </strong>
              </div>
            </div>

            {(state.toolchainStatus.node.warning || state.toolchainStatus.npm.warning) && (
              <div className="notice">
                <AlertCircle size={16} />
                <span>{state.toolchainStatus.node.warning ?? state.toolchainStatus.npm.warning}</span>
              </div>
            )}

            {state.codexStatus.warning && (
              <div className="notice">
                <AlertCircle size={16} />
                <span>{state.codexStatus.warning}</span>
              </div>
            )}

            {!state.toolchainStatus.npm.installed && (
              <button className="updateButton" onClick={installNodejs} disabled={busy}>
                {toolRunning === "安装 Node.js LTS / npm" ? <Loader2 className="spin" size={16} /> : <ChevronRight size={16} />}
                安装 Node.js LTS / npm
              </button>
            )}

            {state.toolchainStatus.npm.installed && !state.codexStatus.localVersion && (
              <button className="updateButton" onClick={installCodex} disabled={busy}>
                {toolRunning === "安装 Codex CLI" ? <Loader2 className="spin" size={16} /> : <ChevronRight size={16} />}
                安装 Codex CLI
              </button>
            )}

            {state.codexStatus.updateAvailable && (
              <button className="updateButton" onClick={updateCodex} disabled={busy}>
                {toolRunning === "更新 Codex" ? <Loader2 className="spin" size={16} /> : <ChevronRight size={16} />}
                更新 Codex
              </button>
            )}

            {(toolRunning || toolLogs.length > 0) && (
              <div className="toolLogPanel">
                <div className="toolLogHeader">
                  <span>
                    <Terminal size={15} />
                    {toolRunning ?? "最近一次安装日志"}
                  </span>
                  <button onClick={() => setToolLogs([])} disabled={busy && Boolean(toolRunning)} title="清空日志">
                    <Trash2 size={14} />
                  </button>
                </div>
                <pre>{toolLogs.join("\n")}</pre>
              </div>
            )}
          </section>

          <section className="panel profilePanel">
            <div className="panelHeader">
              <div>
                <p className="eyebrow">Profile</p>
                <h3>{state.profileStatus.exists ? "已存在" : "待创建"}</h3>
              </div>
              <ShieldCheck size={20} />
            </div>

            <div className="checkList">
              <div className={state.profileStatus.hasProxyBlock ? "checkLine ok" : "checkLine warn"}>
                {state.profileStatus.hasProxyBlock ? <Check size={15} /> : <AlertCircle size={15} />}
                <span>proxy block</span>
              </div>
              <div className={state.profileStatus.hasProjectRootsBlock ? "checkLine ok" : "checkLine warn"}>
                {state.profileStatus.hasProjectRootsBlock ? <Check size={15} /> : <AlertCircle size={15} />}
                <span>project roots block</span>
              </div>
              <div className={state.profileStatus.missingHelpers.length === 0 ? "checkLine ok" : "checkLine warn"}>
                {state.profileStatus.missingHelpers.length === 0 ? <Check size={15} /> : <AlertCircle size={15} />}
                <span>{helperSummary}</span>
              </div>
            </div>
          </section>

          <aside className="panel settingsPanel">
            <div className="panelHeader">
              <div>
                <p className="eyebrow">Settings</p>
                <h3>代理与项目</h3>
              </div>
              <button onClick={() => void chooseProjectRoot()} disabled={busy} title="添加项目根目录">
                <Plus size={16} />
              </button>
            </div>

            <label>
              <span>Proxy URL</span>
              <input value={state.settings.proxyUrl} onChange={(event) => updateSettings({ proxyUrl: event.target.value })} />
            </label>

            <label className="checkOption">
              <input
                type="checkbox"
                checked={state.settings.useProxyForTools}
                onChange={(event) => updateSettings({ useProxyForTools: event.target.checked })}
              />
              <span>安装/更新 npm 与 Codex 时使用此代理</span>
            </label>

            <div className="projectRootList">
              {state.settings.projectRoots.length ? (
                state.settings.projectRoots.map((root) => (
                  <div className="projectRootItem" key={root}>
                    <FolderOpen size={15} />
                    <span title={root}>{root}</span>
                    <button onClick={() => deleteProjectRoot(root)} title="删除项目根目录">
                      <Trash2 size={14} />
                    </button>
                  </div>
                ))
              ) : (
                <div className="emptyList">还没有项目根目录</div>
              )}
            </div>

            <button className="addRootButton" onClick={() => void chooseProjectRoot()} disabled={busy}>
              <Plus size={16} />
              选择项目根目录
            </button>
          </aside>
        </section>
      </main>

      <footer className="statusbar">
        <Check size={15} />
        <span>{status}</span>
      </footer>
    </div>
  );
}

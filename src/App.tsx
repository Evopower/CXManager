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
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import appIcon from "./assets/app-icon.svg";
import { invokeCommand } from "./api";
import { AppSettings, AppState, CodexStatus, RepairResult, SaveResult, TargetShell, UpdateResult } from "./types";

const targetShellLabels: Record<TargetShell, string> = {
  auto: "自动",
  pwsh: "PowerShell 7",
  powershell: "Windows PowerShell 5.1"
};

function statusClass(ok: boolean) {
  return ok ? "statusPill ok" : "statusPill warn";
}

function formatVersion(status: CodexStatus) {
  if (!status.localVersion) return "未检测到";
  if (!status.latestVersion) return status.localVersion;
  return `${status.localVersion} / ${status.latestVersion}`;
}

export default function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("正在加载 CX-Manager");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void loadState();
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

  async function refreshCodex() {
    if (!state) return;
    setBusy(true);
    setError(null);
    try {
      const codexStatus = await invokeCommand<CodexStatus>("refresh_codex_status", {
        proxyUrl: state.settings.proxyUrl
      });
      setState({ ...state, codexStatus });
      setStatus(codexStatus.message);
    } catch (err) {
      setError(String(err));
      setStatus("刷新 Codex 状态失败");
    } finally {
      setBusy(false);
    }
  }

  async function updateCodex() {
    if (!state) return;
    setBusy(true);
    setError(null);
    try {
      const result = await invokeCommand<UpdateResult>("update_codex", {
        proxyUrl: state.settings.proxyUrl
      });
      setState({ ...state, codexStatus: result.codexStatus });
      setStatus(result.stdout || result.message);
    } catch (err) {
      setError(String(err));
      setStatus("Codex 更新失败");
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
              <button onClick={refreshCodex} disabled={busy} title="刷新 Codex 状态">
                <RefreshCw size={16} />
              </button>
            </div>

            <div className="statusRows">
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

            {state.codexStatus.warning && (
              <div className="notice">
                <AlertCircle size={16} />
                <span>{state.codexStatus.warning}</span>
              </div>
            )}

            {state.codexStatus.updateAvailable && (
              <button className="updateButton" onClick={updateCodex} disabled={busy}>
                <ChevronRight size={16} />
                更新 Codex
              </button>
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

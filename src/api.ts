import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { AppState, CodexStatus, RepairResult, RuntimeStatus, SaveResult, ToolchainStatus, UpdateResult } from "./types";

const isTauriRuntime = () => "__TAURI_INTERNALS__" in window;
const demoHome = "C:\\Users\\Example";

const demoCodexStatus: CodexStatus = {
  executablePaths: ["C:\\Program Files\\nodejs\\codex.cmd"],
  localVersion: "0.136.0",
  latestVersion: "0.137.0",
  updateAvailable: true,
  warning: null,
  message: "Codex 可更新: 0.136.0 -> 0.137.0"
};

const demoToolchainStatus: ToolchainStatus = {
  node: {
    installed: true,
    version: "v22.20.0",
    executablePaths: ["C:\\Program Files\\nodejs\\node.exe"],
    warning: null,
    message: "已检测到 Node.js"
  },
  npm: {
    installed: true,
    version: "10.9.3",
    executablePaths: ["C:\\Program Files\\nodejs\\npm.cmd"],
    warning: null,
    message: "已检测到 npm"
  }
};

const demoState: AppState = {
  settings: {
    targetShell: "pwsh",
    proxyUrl: "http://10.20.34.92:7890",
    useProxyForTools: true,
    projectRoots: [`${demoHome}\\PycharmProjects`]
  },
  shellStatus: {
    targetShell: "pwsh",
    command: "pwsh",
    version: "7.5.4",
    profilePath: `${demoHome}\\Documents\\PowerShell\\Microsoft.PowerShell_profile.ps1`,
    message: "当前目标 Shell: pwsh"
  },
  profileStatus: {
    profilePath: `${demoHome}\\Documents\\PowerShell\\Microsoft.PowerShell_profile.ps1`,
    exists: true,
    hasProxyBlock: true,
    hasProjectRootsBlock: true,
    missingHelpers: [],
    isComplete: true,
    message: "Profile 已包含 CX-Manager 必要配置"
  },
  toolchainStatus: demoToolchainStatus,
  codexStatus: demoCodexStatus
};

export async function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauriRuntime()) {
    return tauriInvoke<T>(command, args);
  }

  await new Promise((resolve) => window.setTimeout(resolve, 160));

  if (command === "load_app_state") {
    return demoState as T;
  }

  if (command === "save_app_state") {
    return {
      message: "浏览器预览模式：保存命令只在 Tauri 桌面端执行",
      shellStatus: demoState.shellStatus,
      profileStatus: demoState.profileStatus
    } satisfies SaveResult as T;
  }

  if (command === "repair_profile") {
    return {
      message: "浏览器预览模式：修复命令只在 Tauri 桌面端执行",
      changed: false,
      profileStatus: demoState.profileStatus
    } satisfies RepairResult as T;
  }

  if (command === "refresh_runtime_status") {
    return {
      toolchainStatus: demoToolchainStatus,
      codexStatus: demoCodexStatus
    } satisfies RuntimeStatus as T;
  }

  if (command === "update_codex" || command === "install_codex" || command === "install_nodejs") {
    return {
      message: "浏览器预览模式：安装和更新命令只在 Tauri 桌面端执行",
      stdout: "",
      stderr: "",
      toolchainStatus: demoToolchainStatus,
      codexStatus: demoCodexStatus
    } satisfies UpdateResult as T;
  }

  throw new Error(`Unknown command in browser preview: ${command}`);
}

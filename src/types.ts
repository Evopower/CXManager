export type TargetShell = "auto" | "pwsh" | "powershell";

export type AppSettings = {
  targetShell: TargetShell;
  proxyUrl: string;
  projectRoots: string[];
};

export type ShellStatus = {
  targetShell: TargetShell;
  command: string;
  version: string | null;
  profilePath: string;
  message: string;
};

export type ProfileStatus = {
  profilePath: string;
  exists: boolean;
  hasProxyBlock: boolean;
  hasProjectRootsBlock: boolean;
  missingHelpers: string[];
  isComplete: boolean;
  message: string;
};

export type CodexStatus = {
  executablePaths: string[];
  localVersion: string | null;
  latestVersion: string | null;
  updateAvailable: boolean;
  warning: string | null;
  message: string;
};

export type AppState = {
  settings: AppSettings;
  shellStatus: ShellStatus;
  profileStatus: ProfileStatus;
  codexStatus: CodexStatus;
};

export type SaveResult = {
  message: string;
  shellStatus: ShellStatus;
  profileStatus: ProfileStatus;
};

export type RepairResult = {
  message: string;
  changed: boolean;
  profileStatus: ProfileStatus;
};

export type UpdateResult = {
  message: string;
  stdout: string;
  stderr: string;
  codexStatus: CodexStatus;
};

import { invoke } from "@tauri-apps/api/core";
import { text } from "../strings";

export interface SvnProbe {
  installed: boolean;
  path: string | null;
  guiInstalled: boolean;
  guiPath: string | null;
  version: string | null;
  reason: string | null;
}
export interface SvnSession {
  connected: boolean;
  url: string | null;
  username: string | null;
  remembered: boolean;
}
export interface SvnInspection {
  root: string;
  wcRoot: string;
  url: string;
  repository: string;
  revision: string;
}
export interface SvnStatusEntry {
  path: string;
  local: string;
  properties: string | null;
  remote: string | null;
  remoteProperties: string | null;
  lockOwner: string | null;
  wcLocked: boolean;
  workingCopyLocked: boolean;
  needsLock: boolean;
  remoteOnly: boolean;
}
export interface SvnStatus {
  info: SvnInspection;
  entries: SvnStatusEntry[];
  serverRevision: string | null;
  serverError: string | null;
  recovery: "cleanupRequired" | "resumeRequired" | null;
  updateBlock: string | null;
}
export interface SvnCommitCandidate {
  path: string;
  name: string | null;
  local: string;
  properties: string | null;
  held: boolean;
  newPath: boolean;
  canScheduleDelete: boolean;
  changed: boolean;
  eligible: boolean;
  blockedReason: string | null;
  required: string[];
}
export interface SvnCommitResult {
  revision: string;
  paths: string[];
  deleted: string[];
  unlockPending: string[];
  verificationUnknown: { path: string; check: string; reason: string }[];
}
export interface SvnRegisterPreview {
  root: string;
  url: string;
  directories: string[];
  files: string[];
  excludedFiles: number;
  fingerprint: string;
}
export interface SvnRegisterResult {
  root: string;
  url: string;
  setupRevision: string;
  revision: string;
  files: number;
}
export interface SvnDocumentLockInfo {
  locked: boolean;
  owner: string | null;
  observation: string | null;
}
const errors = {
  svn_cli_missing: "svn.errorMissingCli",
  svn_cli_missing_or_unsupported: "svn.errorMissingCli",
  svn_gui_missing: "svn.errorMissingGui",
  svn_gui_launch_failed: "svn.errorGuiLaunch",
  svn_https_required: "svn.errorHttps",
  svn_invalid_url: "svn.errorUrl",
  svn_credentials_invalid: "svn.errorCredentials",
  svn_auth_failed: "svn.errorAuth",
  svn_tls_failed: "svn.errorTls",
  svn_connection_failed: "svn.errorConnection",
  svn_login_required: "svn.errorLoginRequired",
  svn_other_server: "svn.errorOtherServer",
  svn_destination_not_empty: "svn.errorDestinationNotEmpty",
  svn_destination_unavailable: "svn.errorDestination",
  svn_checkout_may_be_partial: "svn.errorCheckoutPartial",
  svn_checkout_applied_unverified: "svn.errorCheckoutUnverified",
  svn_not_working_copy: "svn.errorWorkingCopy",
  svn_close_project_first: "svn.errorCloseFirst",
  svn_dirty_working_copy: "svn.errorDirty",
  svn_server_unconfirmed: "svn.errorServerUnconfirmed",
  svn_update_may_be_partial: "svn.errorUpdatePartial",
  svn_update_applied_unverified: "svn.errorUpdateUnverified",
  svn_working_copy_cleanup_required: "svn.cleanupRequired",
  svn_working_copy_incomplete: "svn.incompleteBlocked",
  svn_conflicted_working_copy: "svn.conflictBlocked",
  svn_cleanup_not_required: "svn.cleanupNotRequired",
  svn_cleanup_unverified: "svn.cleanupUnverified",
  svn_cancelled: "svn.errorCancelled",
  svn_timeout: "svn.errorTimeout",
  svn_busy: "svn.errorBusy",
  svn_logout_incomplete: "svn.errorLogout",
  svn_release_lock_first: "svn.errorReleaseLockFirst",
  svn_invalid_target: "svn.errorInvalidTarget",
  svn_other_working_copy: "svn.errorWorkingCopy",
  svn_commit_invalid: "svn.errorCommitInvalid",
  svn_commit_selection_changed: "svn.errorCommitSelection",
  svn_commit_lock_required: "svn.errorCommitLockRequired",
  svn_commit_no_changes: "svn.errorCommitNoChanges",
  svn_commit_dependency_missing: "svn.errorCommitDependency",
  svn_commit_unverified: "svn.errorCommitUnverified",
  svn_commit_applied_unverified: "svn.errorCommitAppliedUnverified",
  svn_commit_local_partial: "svn.registerLocalPartial",
  svn_commit_excluded_versioned: "svn.commitExcludedVersioned",
  svn_delete_not_missing: "svn.deleteNotMissing",
  svn_delete_unverified: "svn.deleteUnverified",
  svn_register_target_invalid: "svn.registerTargetInvalid",
  svn_register_source_invalid: "svn.registerSourceInvalid",
  svn_share_invalid: "svn.registerSourceInvalid",
  svn_share_dependency_missing: "svn.registerDependencyMissing",
  svn_register_setup_unverified: "svn.registerSetupUnknown",
  svn_register_path_exists: "svn.registerPathExists",
  svn_register_resume_denied: "svn.registerResumeDenied",
  svn_register_checkout_unverified: "svn.registerCheckoutUnknown",
  svn_register_local_partial: "svn.registerLocalPartial",
  svn_register_commit_unverified: "svn.registerCommitUnknown",
  svn_register_applied_unverified: "svn.registerAppliedUnknown",
  svn_force_invalid_request: "svn.forceInvalidRequest",
  svn_force_context_changed: "svn.forceContextChanged",
  svn_lock_unverified: "svn.forceUnverified",
  svn_status_unsafe: "svn.forceUnsafe",
} as const;
export function svnFailure(error: unknown): string {
  const code = typeof error === "string" ? error : "";
  const key = errors[code as keyof typeof errors];
  return text(key ?? "svn.errorGeneral");
}
export const svnClient = {
  probe: (path?: string, guiPath?: string) =>
    invoke<SvnProbe>("svn_probe", {
      path: path || null,
      guiPath: guiPath || null,
    }),
  session: () => invoke<SvnSession>("svn_session"),
  login: (
    request: string,
    url: string,
    username: string,
    password: string,
    remember: boolean,
  ) =>
    invoke<SvnInspection>("svn_login", {
      request,
      url,
      username,
      password,
      remember,
    }),
  logout: () => invoke<void>("svn_logout"),
  inspect: (request: string, path: string) =>
    invoke<SvnInspection>("svn_inspect", { request, path }),
  checkout: (request: string, url: string, destination: string) =>
    invoke<SvnInspection>("svn_checkout", { request, url, destination }),
  status: (request: string, path: string) =>
    invoke<SvnStatus>("svn_status", { request, path }),
  localDocumentStatus: (path: string, document: string) =>
    invoke<SvnStatusEntry>("svn_local_document_status", { path, document }),
  documentLockOwner: (path: string, document: string) =>
    invoke<SvnDocumentLockInfo>("svn_document_lock_owner", { path, document }),
  forceDocumentLock: (
    path: string,
    document: string,
    expectedOwner: string,
    expectedObservation: string,
    expectedUsername: string,
    reason: string,
  ) =>
    invoke<void>("svn_force_document_lock", {
      path,
      document,
      expectedOwner,
      expectedObservation,
      expectedUsername,
      reason,
    }),
  update: (request: string, path: string) =>
    invoke<SvnStatus>("svn_gui_update", { request, path }),
  cleanup: (request: string, path: string) =>
    invoke<SvnStatus>("svn_gui_cleanup", { request, path }),
  cancel: (request: string) => invoke<boolean>("svn_cancel", { request }),
  commitCandidates: (path: string) =>
    invoke<SvnCommitCandidate[]>("svn_commit_candidates", { path }),
  scheduleDelete: (path: string, target: string) =>
    invoke<void>("svn_schedule_delete", { path, target }),
  commit: (request: string, path: string, paths: string[], message: string) =>
    invoke<SvnCommitResult>("svn_commit", { request, path, paths, message }),
  commitRecheck: (path: string, result: SvnCommitResult) =>
    invoke<SvnCommitResult>("svn_commit_recheck", {
      path,
      revision: result.revision,
      paths: result.paths,
      deleted: result.deleted,
    }),
  registerPreview: (request: string, root: string, url: string) =>
    invoke<SvnRegisterPreview>("svn_register_preview", { request, root, url }),
  register: (
    request: string,
    root: string,
    url: string,
    message: string,
    fingerprint: string,
    resumeEmptyChild: boolean,
  ) =>
    invoke<SvnRegisterResult>("svn_register", {
      request,
      root,
      url,
      message,
      fingerprint,
      resumeEmptyChild,
    }),
};

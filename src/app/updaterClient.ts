import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface UpdateStatus {
  revision: number;
  testMode: boolean;
  installTest?: boolean;
  distribution: "github" | "store" | "dev";
  currentVersion: string;
  channel: "stable" | "test";
  phase:
    | "idle"
    | "checking"
    | "available"
    | "latest"
    | "unavailable"
    | "downloading"
    | "cancelled"
    | "failed"
    | "ready"
    | "preparing"
    | "installing"
    | "install_failed"
    | "skipped";
  candidate: string | null;
  version: string | null;
  notes: string;
  downloaded: number;
  total: number | null;
  error: string | null;
  notify: boolean;
  startupComplete: boolean;
  releaseUrl: string | null;
  cancelPending?: boolean;
  handoff: null | {
    schemaVersion: number;
    candidate: string;
    recovery: unknown[];
    pendingSvn: {
      operationId: string;
      projectFingerprint: string;
      requestedPaths: string[];
      outcome: string;
      observed: unknown;
    }[];
  };
}

/** 시작 선택과 후보는 native가 승인한다. 한 번의 선택만 다운로드와 종료 인계를 잇는다. */
export class UpdateController {
  private value: UpdateStatus | null = null;
  private listeners = new Set<() => void>();
  private initialized: Promise<void> | null = null;
  private started = false;
  private flow: Promise<void> | null = null;
  private flowEpoch = 0;
  private cancelling: Promise<void> | null = null;
  private checking: Promise<void> | null = null;
  private startup: Promise<void> | null = null;
  private releaseStartup: (() => void) | null = null;
  snapshot = () => this.value;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };
  private accept = (value: UpdateStatus) => {
    if (!value || !Number.isSafeInteger(value.revision) || !value.phase) return;
    if (this.value && value.revision < this.value.revision) return;
    this.value = { ...value, cancelPending: this.cancelling !== null };
    if (value.startupComplete) this.releaseStartup?.();
    this.listeners.forEach((listener) => listener());
  };
  initialize() {
    this.initialized ??= (async () => {
      await listen<UpdateStatus>("updater-state", (event) =>
        this.accept(event.payload),
      );
      this.accept(await invoke<UpdateStatus>("updater_status"));
    })().catch(() => {
      this.initialized = null;
      if (!this.value && Reflect.has(window, "__TAURI_INTERNALS__"))
        this.accept({
          revision: 0,
          testMode: false,
          distribution: "dev",
          currentVersion: "—",
          channel: "stable",
          phase: "failed",
          candidate: null,
          version: null,
          notes: "",
          downloaded: 0,
          total: null,
          error: "protocol",
          notify: true,
          startupComplete: false,
          releaseUrl: null,
          handoff: null,
        });
    });
    return this.initialized;
  }
  start() {
    this.startup ??= new Promise<void>((resolve) => {
      this.releaseStartup = resolve;
    });
    if (!this.started) {
      this.started = true;
      void (async () => {
        await this.initialize();
        if (this.value?.startupComplete) {
          this.releaseStartup?.();
          return;
        }
        if (this.value?.error === "protocol") return;
        if (
          this.value?.distribution === "github" ||
          (this.value?.distribution === "dev" && this.value.testMode)
        ) {
          await this.check(false);
          if (
            this.value?.phase === "latest" ||
            this.value?.phase === "unavailable"
          )
            await this.continue();
        } else await this.continue();
      })();
    }
    return this.startup;
  }
  private check(retry: boolean) {
    this.checking ??= (async () => {
      try {
        this.accept(await invoke<UpdateStatus>("updater_check", { retry }));
      } catch {
        this.localFailure("network");
      }
    })().finally(() => {
      this.checking = null;
    });
    return this.checking;
  }
  update() {
    const candidate = this.value?.candidate;
    if (!candidate || this.value?.startupComplete || this.cancelling)
      return Promise.resolve();
    if (this.flow) return this.flow;
    const epoch = ++this.flowEpoch;
    const flow = (async () => {
      try {
        const downloaded = await invoke<UpdateStatus>("updater_download", {
          candidate,
        });
        if (epoch !== this.flowEpoch) return;
        this.accept(downloaded);
        if (
          epoch !== this.flowEpoch ||
          this.value?.startupComplete ||
          this.value?.candidate !== candidate ||
          this.value?.phase !== "ready"
        )
          return;
        this.accept(
          await invoke<UpdateStatus>("updater_prepare", { candidate }),
        );
      } catch (error) {
        if (epoch === this.flowEpoch)
          this.localFailure(typeof error === "string" ? error : "network");
      }
    })().finally(() => {
      if (this.flow === flow) this.flow = null;
    });
    this.flow = flow;
    return flow;
  }
  async retry() {
    if (this.value?.startupComplete) return;
    if (this.value?.error === "protocol") {
      await this.initialize();
      if (this.value?.error === "protocol") return;
    }
    if (this.value?.candidate) await this.update();
    else await this.check(this.value?.phase === "failed");
    if (this.value?.phase === "latest" || this.value?.phase === "unavailable")
      await this.continue();
  }
  cancel() {
    if (this.cancelling) return this.cancelling;
    const candidate = this.value?.candidate;
    if (
      !candidate ||
      this.value?.startupComplete ||
      !["available", "downloading", "ready"].includes(this.value?.phase ?? "")
    )
      return Promise.resolve();
    ++this.flowEpoch;
    // Detach the cancelled continuation. Its finally must not clear a newer
    // explicitly consented flow, even if the old response arrives last.
    this.flow = null;
    const cancellation = (async () => {
      try {
        this.accept(
          await invoke<UpdateStatus>("updater_cancel", { candidate }),
        );
      } catch {
        await this.refresh();
      }
    })().finally(() => {
      if (this.cancelling === cancellation) {
        this.cancelling = null;
        if (this.value) this.accept(this.value);
      }
    });
    this.cancelling = cancellation;
    if (this.value) this.accept(this.value);
    return cancellation;
  }
  async continue() {
    ++this.flowEpoch;
    try {
      this.accept(await invoke<UpdateStatus>("updater_continue"));
    } catch {
      this.localFailure("continue_failed");
    }
  }
  async refresh() {
    try {
      this.accept(await invoke<UpdateStatus>("updater_status"));
    } catch {
      /* 다음 명시 조회에서 다시 확인한다. */
    }
  }
  async openRelease() {
    const candidate = this.value?.candidate;
    if (!candidate) return;
    try {
      await invoke("updater_release", { candidate });
    } catch {
      this.localFailure("link");
    }
  }
  closeFailed() {
    return invoke("updater_close_failed");
  }
  private localFailure(error: string) {
    if (!this.value) return;
    this.accept({ ...this.value, phase: "failed", error, notify: true });
  }
}
const singleton = new UpdateController();
export const updaterController = () => singleton;

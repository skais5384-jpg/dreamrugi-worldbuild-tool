import { FloatingNotice, FloatingNoticeContent } from "../ui/FloatingNotice";
import { Field, Label } from "@fluentui/react-components";
import { Button, Input, Textarea } from "../ui/Controls";
import { HelpText } from "../ui/HelpText";
import { EmptyState } from "../ui/EmptyState";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { appController, type TemplateController } from "./controller";
import { Confirm } from "./Confirm";
import { ActivityLog, useActivityLog } from "./ActivityLog";
import { FollowUp } from "./FollowUp";
import { Fields } from "./Fields";
import { TemplateManagement } from "./TemplateManagement";
import { projectLabel, lifecycleLabel } from "./statusLabels";
import { text } from "../strings";
import { useImeGuard } from "./ime";
import { SvnDialog } from "./SvnDialog";
import "./App.css";

function App({
  controller = appController(),
}: {
  controller?: TemplateController;
}) {
  const state = useSyncExternalStore(controller.subscribe, controller.snapshot);
  const activityLog = useActivityLog(true);
  const [logOpen, setLogOpen] = useState(false);
  const [logNoticeKey, setLogNoticeKey] = useState<string>();
  useEffect(() => {
    const open = (event: Event) => {
      setLogNoticeKey(
        event instanceof CustomEvent ? event.detail?.noticeKey : undefined,
      );
      setLogOpen(true);
    };
    window.addEventListener("open-activity-log", open);
    return () => window.removeEventListener("open-activity-log", open);
  }, []);
  const nameInput = useRef<HTMLTextAreaElement>(null);
  const rootInput = useRef<HTMLInputElement>(null);
  const browse = useRef<HTMLButtonElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const prompted = useRef(false);
  const projectIme = useImeGuard();
  const [svnEntry, setSvnEntry] = useState<"connect" | "checkout" | null>(null);
  const nameIme = useImeGuard();
  const formKind = state.form?.kind;
  useEffect(() => {
    controller.start();
  }, [controller]);
  useEffect(() => {
    if (formKind && !state.busy && !state.prompt) nameInput.current?.focus();
  }, [formKind, state.busy, state.prompt]);
  useEffect(() => {
    // 확인창에는 DialogTrigger가 없다. inert/disabled 해제 뒤 기억한 실제 입력으로 복원한다.
    if (prompted.current && !state.prompt && returnFocus.current?.isConnected)
      returnFocus.current.focus();
    prompted.current = !!state.prompt;
  }, [state.prompt]);
  const locked =
    state.busy || !!state.prompt || state.deciding || state.closing;
  const submissionPending =
    state.templateAction?.phase === "pending" ||
    (!!state.templateAction?.handedOff && state.retainedRefs.length > 0) ||
    !!state.form?.submitted ||
    !!state.fieldEditor?.drafts.some((d) => d.submitted && !d.committed);
  const readyProject =
    state.project?.status === "Ready" && state.project.runtime === "Ready";
  const canEdit =
    readyProject &&
    !state.project?.collaborative &&
    state.ready &&
    !state.sessions.some((s) => s.problem) &&
    !state.retainedRefs.length;
  return (
    <main className="app-shell">
      <div
        inert={!!state.prompt}
        onFocusCapture={(event) => {
          // disabled/inert 적용으로 브라우저가 focus를 해제하기 전에 복귀 대상을 기억한다.
          returnFocus.current = event.target;
        }}
      >
        <header className="app-toolbar">
          <h1>{text("app.message02")}</h1>
          {state.project && (
            <section className="project-bar" aria-label={text("app.message05")}>
              <strong>{projectLabel(state.project, state.closing)}</strong>
              <div className="actions">
                <Button
                  type="button"
                  disabled={locked || submissionPending}
                  onClick={() =>
                    void controller.navigate({ kind: "close_project" })
                  }
                >
                  {text("app.message10")}
                </Button>
                {!readyProject && (
                  <Button
                    type="button"
                    disabled={locked}
                    onClick={() => void controller.recover()}
                  >
                    {text("app.message11")}
                  </Button>
                )}
                {state.project.error?.code === "initialization_failed" && (
                  <Button
                    type="button"
                    disabled={locked}
                    onClick={() =>
                      void controller.correctProjectPath().then((corrected) => {
                        if (corrected)
                          requestAnimationFrame(() =>
                            rootInput.current?.focus(),
                          );
                      })
                    }
                  >
                    {text("project.correctPath")}
                  </Button>
                )}
              </div>
            </section>
          )}
          <div className="actions">
            <Button type="button" onClick={() => void controller.checkStatus()}>
              {text("app.message03")}
            </Button>
            <Button
              type="button"
              onClick={() => void controller.requestClose()}
              disabled={!state.ready || !!state.prompt || state.closing}
            >
              {text("app.message04")}
            </Button>
          </div>
        </header>
        {state.message && <FloatingNotice>{state.message}</FloatingNotice>}
        {state.error && (
          <FloatingNotice intent="error">
            <FloatingNoticeContent>{state.error}</FloatingNoticeContent>
          </FloatingNotice>
        )}
        <FollowUp controller={controller} />
        {!state.project && (
          <section className="project-open" aria-label={text("app.message05")}>
            <div className="actions">
              <Button
                type="button"
                disabled={!state.ready || locked}
                onClick={() => setSvnEntry("checkout")}
              >
                {text("svn.checkout")}
              </Button>
            </div>
            <form
              {...projectIme.bind}
              onSubmit={(event) => {
                event.preventDefault();
                if (projectIme.allowAction(event)) void controller.open();
              }}
            >
              <Label htmlFor="project-root">{text("app.message06")}</Label>
              <div className="actions">
                <Input
                  ref={rootInput}
                  id="project-root"
                  className="project-path"
                  value={state.root}
                  disabled={!state.ready || locked || !!state.projectId}
                  onChange={(event) => controller.setRoot(event.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                />
                <Button
                  type="button"
                  ref={browse}
                  disabled={
                    !state.ready || locked || state.picking || !!state.projectId
                  }
                  onClick={() =>
                    void controller.pickFolder().then((restore) => {
                      if (restore && browse.current?.isConnected)
                        browse.current.focus();
                    })
                  }
                >
                  {text(state.picking ? "picker.pending" : "picker.browse")}
                </Button>
                <Button
                  type="submit"
                  appearance="primary"
                  disabled={
                    !state.ready ||
                    locked ||
                    state.picking ||
                    !state.root ||
                    !!state.projectId
                  }
                >
                  {text("app.message07")}
                </Button>
              </div>
              <p>{text("app.message08")}</p>
              {state.projectId && <p>{text("app.message09")}</p>}
            </form>
          </section>
        )}
        {state.project?.shutdown.joined &&
          state.project.shutdown.normalExitAllowed &&
          !state.closing && (
            <Button
              type="button"
              disabled={locked}
              onClick={() => void controller.retireProject()}
            >
              {text("app.message12")}
            </Button>
          )}
        {state.project && (
          <div className="workspace">
            <section
              className="panel template-list"
              aria-labelledby="templates-title"
            >
              <div className="panel-heading">
                <h2 id="templates-title">{text("app.message13")}</h2>
                <span>{state.rows.length}</span>
              </div>
              <div className="actions">
                <Button
                  type="button"
                  appearance="primary"
                  disabled={locked || !canEdit || submissionPending}
                  onClick={() => void controller.navigate({ kind: "create" })}
                >
                  {text("app.message14")}
                </Button>
                <Button
                  type="button"
                  disabled={locked || !readyProject}
                  onClick={() => void controller.refresh()}
                >
                  {text("app.message15")}
                </Button>
              </div>
              {state.listState === "loading" && <p>{text("app.message16")}</p>}
              {state.listState === "failed" && (
                <p className="error">{text("app.message17")}</p>
              )}
              {state.listState === "ready" && state.rows.length === 0 && (
                <p className="empty">
                  {text("app.message18")}
                  <br />
                  {text("app.message19")}
                </p>
              )}
              <ul>
                {state.rows.map((row) => (
                  <li key={row.id}>
                    <Button
                      type="button"
                      className="template-choice"
                      appearance="subtle"
                      aria-pressed={state.selection?.content.id === row.id}
                      disabled={locked || submissionPending}
                      onClick={() =>
                        void controller.navigate({ kind: "select", id: row.id })
                      }
                    >
                      <strong>{row.name || text("field.emptyLabel")}</strong>
                      <span>
                        {text("template.version", {
                          revision: row.revision,
                          lifecycle: lifecycleLabel(row.lifecycle),
                        })}
                      </span>
                      <span className="identifier">{row.id}</span>
                    </Button>
                  </li>
                ))}
              </ul>
            </section>
            <section
              className="panel detail"
              aria-label={text("app.message20")}
            >
              {state.templateAction && (
                <TemplateManagement
                  key={state.templateAction.generation}
                  controller={controller}
                  locked={locked}
                />
              )}
              {state.form && (
                <form
                  {...nameIme.bind}
                  className="editor"
                  onSubmit={(event) => {
                    event.preventDefault();
                    if (nameIme.allowAction(event)) void controller.save();
                  }}
                >
                  <h2>
                    {state.form.kind === "create"
                      ? text("app.message21")
                      : text("app.message22")}
                  </h2>
                  <Field
                    label={text("app.message23")}
                    hint={
                      <HelpText>
                        {text("app.message24")}{" "}
                        {state.form.source &&
                          text("field.basis", {
                            revision: state.form.source.content.revision,
                          })}
                      </HelpText>
                    }
                  >
                    <Textarea
                      rows={2}
                      ref={nameInput}
                      value={state.form.name}
                      disabled={locked || state.form.submitted}
                      onChange={(event) =>
                        controller.setName(event.target.value)
                      }
                    />
                  </Field>
                  <div className="actions">
                    <Button
                      type="submit"
                      appearance="primary"
                      disabled={locked || state.form.submitted}
                    >
                      {text("app.message25")}
                    </Button>
                    <Button
                      type="button"
                      disabled={locked || state.form.submitted}
                      onClick={() =>
                        void controller.navigate({ kind: "cancel" })
                      }
                    >
                      {text("app.message26")}
                    </Button>
                  </div>
                  {state.form.submitted && <p>{text("app.message27")}</p>}
                </form>
              )}
              {state.selection ? (
                <>
                  <div className="panel-heading">
                    <div>
                      <p className="eyebrow">{text("app.message28")}</p>
                      <h2>
                        {state.selection.content.name ||
                          text("field.emptyLabel")}
                      </h2>
                    </div>
                    <div className="actions">
                      <Button
                        type="button"
                        disabled={
                          locked ||
                          !canEdit ||
                          submissionPending ||
                          !!state.templateAction
                        }
                        onClick={() =>
                          void controller.navigate({ kind: "duplicate" })
                        }
                      >
                        {text("template.duplicate")}
                      </Button>
                      {state.selection.content.lifecycle === "Active" && (
                        <Button
                          type="button"
                          danger
                          disabled={
                            locked ||
                            !canEdit ||
                            submissionPending ||
                            !!state.templateAction
                          }
                          onClick={() =>
                            void controller.navigate({ kind: "delete" })
                          }
                        >
                          {text("template.delete")}
                        </Button>
                      )}
                      {state.selection.content.lifecycle === "Active" && (
                        <Button
                          type="button"
                          disabled={
                            locked ||
                            !canEdit ||
                            !!state.form ||
                            !!state.fieldEditor ||
                            !!state.templateAction
                          }
                          onClick={() => void controller.rename()}
                        >
                          {text("app.message22")}
                        </Button>
                      )}
                    </div>
                  </div>
                  <p>
                    {text("template.version", {
                      revision: state.selection.content.revision,
                      lifecycle: lifecycleLabel(
                        state.selection.content.lifecycle,
                      ),
                    })}
                  </p>
                  <p className="identifier">
                    {text("template.resultId", {
                      id: state.selection.content.id,
                    })}
                  </p>
                  {!state.form && !state.templateAction && (
                    <Fields
                      controller={controller}
                      locked={locked}
                      canEdit={canEdit}
                    />
                  )}
                </>
              ) : (
                !state.form && <EmptyState>{text("app.message31")}</EmptyState>
              )}
            </section>
          </div>
        )}
      </div>
      {/* Dialog를 유지해야 닫힘 뒤 Tabster가 배경 접근성을 복구할 수 있다. */}
      <Confirm controller={controller} />
      {logOpen && (
        <ActivityLog
          noticeKey={logNoticeKey}
          events={activityLog.events}
          droppedEvents={activityLog.droppedEvents}
          close={() => setLogOpen(false)}
        />
      )}
      <SvnDialog
        open={svnEntry !== null}
        entry={svnEntry ?? "connect"}
        onClose={() => setSvnEntry(null)}
        onSessionChanged={() => undefined}
        controller={controller}
      />
    </main>
  );
}

export default App;

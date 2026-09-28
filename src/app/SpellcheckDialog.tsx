import { invoke } from "@tauri-apps/api/core";
import {
  Badge,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
} from "@fluentui/react-components";
import { CheckmarkCircle20Filled } from "@fluentui/react-icons";
import { useEffect, useRef, useState } from "react";
import { Button, Select } from "../ui/Controls";
import type { DocumentController } from "./documentController";
import type { EditEntry } from "./documentEdits";
import {
  collectSurfaces,
  glossaryTerms,
  tokens,
  type SpellFinding,
} from "./spellcheck";
import { applySpellFindings, remainingSpellFindings } from "./spellApply";
import { text, type StringKey } from "../strings";

interface WordResult {
  word: string;
  valid: boolean;
  suggestions: string[];
}
type Phase = "idle" | "scanning" | "complete" | "cancelled" | "failed";
const spellErrors = [
  "검사 요청 권한이 없습니다.",
  "검사 범위가 올바르지 않습니다.",
  "다른 검사가 진행 중입니다.",
  "검사를 취소했습니다.",
  "검사 작업을 완료할 수 없습니다.",
  "검사기 자료 이름이 올바르지 않습니다.",
  "검사기 위치를 확인할 수 없습니다.",
  "로컬 검사기 파일 형식이 올바르지 않습니다.",
  "로컬 검사기 파일을 읽을 수 없습니다.",
  "로컬 검사기 파일이 배포본과 다릅니다. 앱을 다시 설치해 주세요.",
  "로컬 검사기 파일이 없습니다. 앱 설치를 확인해 주세요.",
  "검사 작업 공간을 확인할 수 없습니다.",
  "검사 작업 공간을 만들 수 없습니다.",
  "검사 작업 공간이 안전하지 않습니다.",
  "검사 입력을 만들 수 없습니다.",
  "검사 입력을 쓸 수 없습니다.",
  "검사 입력을 저장할 수 없습니다.",
  "검사 출력을 만들 수 없습니다.",
  "검사 입력을 읽을 수 없습니다.",
  "로컬 검사기를 시작할 수 없습니다.",
  "검사 작업이 사라졌습니다.",
  "검사 시간이 길어 중단했습니다. 문서를 나눠 다시 검사해 주세요.",
  "검사기 응답을 읽을 수 없습니다.",
  "로컬 검사기가 입력을 처리하지 못했습니다.",
  "검사 결과를 읽을 수 없습니다.",
  "검사 결과가 너무 큽니다. 문서를 나눠 검사해 주세요.",
  "검사 결과 인코딩을 읽을 수 없습니다.",
  "검사 결과 형식이 올바르지 않습니다.",
  "검사 결과와 요청이 일치하지 않습니다.",
] as const;
const spellErrorKeys = [
  "spell.native01",
  "spell.native02",
  "spell.native03",
  "spell.native04",
  "spell.native05",
  "spell.native06",
  "spell.native07",
  "spell.native08",
  "spell.native09",
  "spell.native10",
  "spell.native11",
  "spell.native12",
  "spell.native13",
  "spell.native14",
  "spell.native15",
  "spell.native16",
  "spell.native17",
  "spell.native18",
  "spell.native19",
  "spell.native20",
  "spell.native21",
  "spell.native22",
  "spell.native23",
  "spell.native24",
  "spell.native25",
  "spell.native26",
  "spell.native27",
  "spell.native28",
  "spell.native29",
] as const satisfies readonly StringKey[];
class SpellUiFailure extends Error {
  constructor(readonly key: StringKey) {
    super(key);
  }
}
function spellFailure(
  cause: unknown,
  stage: "glossary" | "documents" | "engine",
) {
  if (cause instanceof SpellUiFailure) return text(cause.key);
  if (typeof cause === "string") {
    const index = spellErrors.findIndex((message) => message === cause);
    if (index >= 0) return text(spellErrorKeys[index]);
  }
  return text(
    stage === "glossary"
      ? "spell.glossaryFailed"
      : stage === "documents"
        ? "spell.documentsFailed"
        : "spell.requestFailed",
  );
}

export function SpellcheckDialog({
  controller,
  document,
  entry,
  locked,
  close,
}: {
  controller: DocumentController;
  document: string;
  entry: EditEntry;
  locked: boolean;
  close: () => void;
}) {
  const [phase, setPhase] = useState<Phase>("idle");
  const [message, setMessage] = useState("");
  const [findings, setFindings] = useState<SpellFinding[]>([]);
  const [hadFindings, setHadFindings] = useState(false);
  const [busyApplying, setBusyApplying] = useState(false);
  const [visibleCount, setVisibleCount] = useState(200);
  const [scope, setScope] = useState("");
  const [owner, setOwner] = useState("");
  const [termSignature, setTermSignature] = useState("");
  const request = useRef<string | null>(null);
  const applying = useRef(false);
  const alive = useRef(true);
  const focus = useRef<HTMLElement | null>(documentElement());
  function documentElement() {
    return typeof window === "undefined"
      ? null
      : (window.document.activeElement as HTMLElement | null);
  }
  const cancel = () => {
    const id = request.current;
    request.current = null;
    if (id) void invoke("spell_cancel", { requestId: id }).catch(() => {});
  };
  useEffect(
    () => () => {
      alive.current = false;
      cancel();
    },
    [],
  );
  const dismiss = () => {
    cancel();
    applying.current = false;
    close();
    requestAnimationFrame(() => focus.current?.focus({ preventScroll: true }));
  };
  const run = async () => {
    if (request.current || locked || entry.body.composing) return;
    const id = crypto.randomUUID();
    request.current = id;
    setPhase("scanning");
    setMessage(text("spell.preparing"));
    setFindings([]);
    setHadFindings(false);
    setVisibleCount(200);
    let stage: "glossary" | "documents" | "engine" = "glossary";
    try {
      const project = controller.shell.snapshot().projectId;
      const generation = controller.shell.projectGeneration();
      await controller.shell.refresh();
      stage = "documents";
      await controller.load();
      if (request.current !== id || !alive.current) return;
      const current = controller.edits.entries[document];
      const list = controller.snapshot().list;
      const shell = controller.shell.snapshot();
      if (
        !project ||
        shell.projectId !== project ||
        controller.shell.projectGeneration() !== generation ||
        !current ||
        current.status.owner !== entry.status.owner ||
        current.body.composing ||
        current.paused ||
        locked ||
        shell.listState !== "ready"
      )
        throw new SpellUiFailure("spell.editChanged");
      const terms = glossaryTerms(list, shell.rows);
      if (!terms) throw new SpellUiFailure("spell.glossaryStale");
      const report = { excluded: 0 };
      const surfaces = collectSurfaces(document, current, report);
      const totalChars = surfaces.reduce(
        (sum, surface) => sum + surface.text.length,
        0,
      );
      if (totalChars > 500_000) throw new SpellUiFailure("spell.tooLong");
      const occurrences = tokens(surfaces, terms);
      if (occurrences.length > 20_000)
        throw new SpellUiFailure("spell.tooManyTokens");
      const words = [...new Set(occurrences.map((item) => item.word))];
      if (words.length > 4096) throw new SpellUiFailure("spell.tooManyWords");
      setMessage(
        text("spell.scanningCount", {
          surfaces: String(surfaces.length),
          words: String(occurrences.length),
        }),
      );
      stage = "engine";
      const results = await invoke<WordResult[]>("spell_check", {
        requestId: id,
        words,
      });
      if (request.current !== id || !alive.current) return;
      const latest = controller.edits.entries[document];
      if (
        !latest ||
        latest.status.owner !== current.status.owner ||
        latest.generation !== current.generation ||
        controller.shell.snapshot().projectId !== project ||
        controller.shell.projectGeneration() !== generation
      )
        throw new SpellUiFailure("spell.changedDuringScan");
      const byWord = new Map(results.map((row) => [row.word, row]));
      const next = occurrences.flatMap((item, index): SpellFinding[] => {
        const result = byWord.get(item.word);
        if (!result || result.valid) return [];
        const suggestions = [...new Set(result.suggestions)].filter(
          (candidate) => candidate !== item.word,
        );
        return [
          {
            id: `${index}:${item.surface.key}:${item.start}`,
            surface: item.surface,
            start: item.start,
            end: item.end,
            original: item.original,
            suggestions,
            selected: suggestions.length === 1 ? suggestions[0] : null,
            ignored: false,
          },
        ];
      });
      setFindings(next);
      setHadFindings(next.length > 0);
      setOwner(current.status.owner);
      setScope(`${generation}:${project}`);
      setTermSignature(JSON.stringify(terms));
      setPhase("complete");
      const partial = report.excluded
        ? text("spell.excluded", { count: String(report.excluded) })
        : "";
      setMessage(
        next.length
          ? text("spell.found", { count: String(next.length) }) + partial
          : text("spell.noFindings") + partial,
      );
    } catch (cause) {
      if (request.current !== id || !alive.current) return;
      setPhase("failed");
      setMessage(spellFailure(cause, stage));
    } finally {
      if (request.current === id) request.current = null;
    }
  };
  const startOnce = useRef(run);
  useEffect(() => {
    const timer = window.setTimeout(() => void startOnce.current(), 0);
    return () => window.clearTimeout(timer);
  }, []);
  const eligible = findings
    .slice(0, visibleCount)
    .filter((finding) => !finding.ignored && !!finding.selected);
  const apply = async (selected: SpellFinding[]) => {
    if (applying.current || locked || entry.body.composing || !selected.length)
      return;
    applying.current = true;
    setBusyApplying(true);
    try {
      await controller.shell.refresh();
      await controller.load();
      const latestTerms = glossaryTerms(
        controller.snapshot().list,
        controller.shell.snapshot().rows,
      );
      if (!latestTerms || JSON.stringify(latestTerms) !== termSignature) {
        setPhase("failed");
        setMessage(text("spell.glossaryChanged"));
        return;
      }
      const result = await applySpellFindings(
        controller,
        document,
        owner,
        scope,
        selected,
        () => alive.current && applying.current,
      );
      if (!alive.current || !applying.current) return;
      setFindings((rows) => remainingSpellFindings(rows, result));
      setMessage(
        result.skipped
          ? text("spell.applyPartial", {
              applied: String(result.applied),
              skipped: String(result.skipped),
            })
          : text("spell.applyDone", { applied: String(result.applied) }),
      );
    } catch {
      setPhase("failed");
      setMessage(text("spell.applyFailed"));
    } finally {
      applying.current = false;
      setBusyApplying(false);
    }
  };
  return (
    <Dialog
      open
      onOpenChange={(_, data) => {
        if (!data.open) dismiss();
      }}
    >
      <DialogSurface className="spellcheck-dialog">
        <DialogBody>
          <DialogTitle>{text("spell.title")}</DialogTitle>
          <DialogContent>
            <p>{text("spell.purpose")}</p>
            <p role="status">{message || text("spell.start")}</p>
            {phase === "complete" && (
              <div className="spellcheck-results">
                {findings.length === 0 &&
                  (hadFindings ? (
                    <div className="spellcheck-complete">
                      <Badge
                        appearance="tint"
                        color="success"
                        icon={<CheckmarkCircle20Filled aria-hidden="true" />}
                      >
                        {text("spell.doneBadge")}
                      </Badge>
                      <span>{text("spell.allHandled")}</span>
                    </div>
                  ) : (
                    <p>{text("spell.noCorrections")}</p>
                  ))}
                {findings.slice(0, visibleCount).map((finding) => (
                  <section key={finding.id} className="spellcheck-card">
                    <h3>{finding.surface.label}</h3>
                    <p className="spellcheck-context">
                      {finding.surface.text.slice(
                        Math.max(0, finding.start - 24),
                        finding.start,
                      )}
                      <strong>{finding.original}</strong>
                      {finding.surface.text.slice(
                        finding.end,
                        finding.end + 24,
                      )}
                    </p>
                    <p>{text("spell.unknownWord")}</p>
                    {finding.suggestions.length ? (
                      <Select
                        aria-label={text("spell.suggestionFor", {
                          location: finding.surface.label,
                          word: finding.original,
                        })}
                        value={finding.selected ?? ""}
                        disabled={busyApplying}
                        onChange={(event) => {
                          if (applying.current) return;
                          setFindings((rows) =>
                            rows.map((row) =>
                              row.id === finding.id
                                ? {
                                    ...row,
                                    selected: event.target.value || null,
                                  }
                                : row,
                            ),
                          );
                        }}
                      >
                        <option value="">
                          {text("spell.chooseSuggestion")}
                        </option>
                        {finding.suggestions.map((suggestion) => (
                          <option key={suggestion} value={suggestion}>
                            {suggestion}
                          </option>
                        ))}
                      </Select>
                    ) : (
                      <p>{text("spell.noSuggestion")}</p>
                    )}
                    <div className="spellcheck-card-actions">
                      <Button
                        type="button"
                        disabled={busyApplying || !finding.selected}
                        onClick={() => void apply([finding])}
                      >
                        {text("spell.apply")}
                      </Button>
                      <Button
                        type="button"
                        disabled={busyApplying}
                        onClick={() => {
                          if (applying.current) return;
                          setFindings((rows) =>
                            rows.filter((row) => row.id !== finding.id),
                          );
                          setMessage(text("spell.ignored"));
                        }}
                      >
                        {text("spell.ignore")}
                      </Button>
                    </div>
                  </section>
                ))}
                {findings.length > visibleCount && (
                  <Button
                    type="button"
                    onClick={() => setVisibleCount((count) => count + 200)}
                  >
                    {text("spell.more", {
                      count: String(findings.length - visibleCount),
                    })}
                  </Button>
                )}
              </div>
            )}
          </DialogContent>
          <DialogActions>
            {phase === "scanning" && (
              <Button
                type="button"
                onClick={() => {
                  cancel();
                  setPhase("cancelled");
                  setMessage(text("spell.cancelled"));
                }}
              >
                {text("spell.cancel")}
              </Button>
            )}
            {phase !== "scanning" && (
              <Button
                type="button"
                onClick={() => void run()}
                disabled={locked || busyApplying}
              >
                {text("spell.retry")}
              </Button>
            )}
            {phase === "complete" && (
              <Button
                type="button"
                disabled={busyApplying || eligible.length === 0}
                onClick={() => void apply(eligible)}
              >
                {text("spell.applyAll")}
              </Button>
            )}
            <Button type="button" onClick={dismiss}>
              {text("spell.close")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

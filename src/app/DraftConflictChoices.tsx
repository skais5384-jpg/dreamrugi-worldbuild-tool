import { useState } from "react";
import { Radio, RadioGroup } from "@fluentui/react-components";
import type { RecoveryChange } from "../bridge/workspace";
import type { Field, Template, Value } from "../bridge/types";
import { ValueRead } from "./FieldValue";
import type { ReferenceContext } from "./DocumentReferenceValue";
import { ExternalMediaPreviewContext, MediaActiveContext } from "./MediaValue";
import { InlineNotice } from "../ui/InlineNotice";
import { PropertyRow } from "./PropertyRow";
import { Button } from "../ui/Controls";
import { text } from "../strings";

const labels: Record<string, string> = {
  name: "이름",
  englishName: "영문 이름",
  glossarySummary: "한 줄 설명",
  glossaryExcluded: "글로서리 표시",
  label: "항목 이름",
  required: "필수 입력",
  writingGuide: "작성 안내",
  presentation: "표시 서식",
  default: "기본값",
  fields: "필드",
  members: "하위 필드",
  options: "선택지",
  sections: "섹션",
  cardTitleField: "카드 제목",
  archived: "보관 상태",
  configuration: "구성",
  kind: "형식",
  minimum: "최솟값",
  maximum: "최댓값",
  $order: "표시 순서",
  value: "내용",
  intent: "내용",
  content: "본문",
  multiple: "여러 문서 연결",
  reciprocalNotice: "역방향 연결 안내",
  allowedTemplates: "연결 가능한 템플릿",
};
const kinds: Record<string, string> = {
  single_line_text: "한 줄 텍스트",
  rich_text: "서식 텍스트",
  number: "숫자",
  date: "날짜",
  time: "시각",
  duration: "시간 길이",
  image: "이미지",
  file: "파일 첨부",
  url: "URL 미디어",
  single_choice: "단일 선택",
  multi_choice: "다중 선택",
  relation: "관계",
  document_link: "문서 링크",
  group: "반복 그룹",
};
function knownFields(template: Template): Field[] {
  const result: Field[] = [];
  const add = (fields: Field[]) =>
    fields.forEach((field) => {
      result.push(field);
      add(field.members ?? []);
    });
  add(template.fields);
  return result;
}
function ValuePreview({
  value,
  field,
  names,
  property,
  reference,
}: {
  value: unknown;
  field?: Field;
  names: Map<string, string>;
  property: string;
  reference?: ReferenceContext;
}) {
  if (value === null || value === undefined)
    return <span>{text("field.unsetValue")}</span>;
  if (typeof value === "boolean") return <span>{value ? "예" : "아니요"}</span>;
  if (typeof value === "number") return <span>{String(value)}</span>;
  if (typeof value === "string") {
    if (property === "intent")
      return (
        <span>
          {value === "unset"
            ? "미작성"
            : value === "keep"
              ? "현재 내용 유지"
              : "입력한 내용 사용"}
        </span>
      );
    if (property === "kind")
      return <span>{kinds[value] ?? "형식 확인 불가"}</span>;
    if (
      [
        "cardTitleField",
        "allowedTemplates",
        "$order",
        "option",
        "options",
        "field",
        "document",
      ].includes(property)
    )
      return <span>{names.get(value) ?? "이름을 확인할 수 없는 항목"}</span>;
    return <span className="literal">{value}</span>;
  }
  if (Array.isArray(value))
    return (
      <ul>
        {value.map((item, index) => (
          <li key={index}>
            <ValuePreview
              value={item}
              field={field}
              names={names}
              property={property}
              reference={reference}
            />
          </li>
        ))}
      </ul>
    );
  if (typeof value !== "object")
    return <span>이 내용을 표시할 수 없어요.</span>;
  const record = value as Record<string, unknown>;
  if ("intent" in record)
    return record.intent === "set" ? (
      <ValuePreview
        value={record.value}
        field={field}
        names={names}
        property={property}
        reference={reference}
      />
    ) : (
      <span>
        {record.intent === "unset"
          ? text("field.unsetValue")
          : "현재 내용 유지"}
      </span>
    );
  if (
    typeof record.kind === "string" &&
    ("value" in record ||
      "content" in record ||
      "links" in record ||
      "documents" in record ||
      "option" in record ||
      "options" in record ||
      "instances" in record ||
      ["unset", "number_unknown"].includes(record.kind))
  )
    return (
      <ValueRead
        value={value as Value}
        options={field?.options ?? []}
        field={field}
        reference={reference}
      />
    );
  const display = Object.entries(record).filter(
    ([key]) => key in labels && key !== "intent",
  );
  return display.length ? (
    <>
      {display.map(([key, item]) => (
        <PropertyRow key={key} label={labels[key]} block>
          <ValuePreview
            value={item}
            field={field}
            names={names}
            property={key}
            reference={reference}
          />
        </PropertyRow>
      ))}
    </>
  ) : (
    <span>지원하지 않는 내용은 원래 입력에 남겨 두었어요.</span>
  );
}
/** Only actual conflicts require a choice. Internal comparison paths never become labels. */
export function DraftConflictChoices({
  changes,
  template,
  busy,
  apply,
  cancel,
  reference,
  values = [],
}: {
  changes: RecoveryChange[];
  template: Template;
  busy: boolean;
  apply: (selected: string[]) => Promise<void>;
  cancel?: () => Promise<void>;
  reference?: ReferenceContext;
  values?: unknown[];
}) {
  const [choices, setChoices] = useState<Record<string, string>>({});
  const fields = knownFields(template);
  const names = new Map([
    ...fields.flatMap((field) => [
      [field.id, field.label] as const,
      ...field.options.map((option) => [option.id, option.label] as const),
    ]),
    [template.id, template.name] as const,
    ...(reference?.templates ?? []).map((row) => [row.id, row.name] as const),
    ...(template.sections ?? []).map(
      (section) => [section.id, section.title] as const,
    ),
  ]);
  // Give cards stable readable labels across both orders; never use their storage IDs as text.
  const addCards = (value: unknown, depth = 0): void => {
    if (!value || depth > 24) return;
    if (Array.isArray(value)) {
      value.forEach((item) => addCards(item, depth + 1));
      return;
    }
    if (typeof value !== "object") return;
    const record = value as Record<string, unknown>;
    if (record.kind === "group" && Array.isArray(record.instances))
      record.instances.forEach((card, index) => {
        if (
          card &&
          typeof card === "object" &&
          typeof card.id === "string" &&
          !names.has(card.id)
        )
          names.set(card.id, `반복 항목 ${index + 1}`);
      });
    Object.values(record).forEach((item) => addCards(item, depth + 1));
  };
  values.forEach((value) => addCards(value));
  changes.forEach((change) =>
    [change.original, change.current, change.preserved].forEach((value) =>
      addCards(value),
    ),
  );
  changes
    .filter(
      (change) =>
        change.path.includes("instances") &&
        change.path[change.path.length - 1] === "$order",
    )
    .forEach((change) => {
      for (const value of [change.original, change.current, change.preserved])
        if (Array.isArray(value))
          value.forEach((id) => {
            if (typeof id === "string" && !names.has(id))
              names.set(
                id,
                `반복 항목 ${1 + [...names.values()].filter((name) => name.startsWith("반복 항목 ")).length}`,
              );
          });
    });
  const conflicts = changes.filter((change) => change.status === "conflict");

  return (
    <section className="draft-conflict-choices">
      <InlineNotice kind="warning">
        현재 저장된 내용과 작성 중 내용이 달라요. 서로 다르게 바뀐 부분만 선택해
        주세요. 다른 변경은 함께 유지합니다.
      </InlineNotice>
      {changes
        .filter((change) => change.status !== "proposed")
        .map((change, index) => {
          const field = [...change.path]
            .reverse()
            .map((id) => fields.find((field) => field.id === id))
            .find(Boolean);
          const identityLabels = change.path
            .filter((id) => names.has(id))
            .map((id) => names.get(id)!);
          const property = change.path[change.path.length - 1] ?? "";
          const label = [
            ...identityLabels,
            labels[property] ?? (names.get(property) ? "내용" : "구조 변경"),
          ]
            .filter(Boolean)
            .join(" · ");
          return (
            <section key={change.id} className="recovery-compare-item">
              <h3>{label}</h3>
              {change.status === "conflict" &&
                field?.kind === "Group" &&
                property === "value" && (
                  <InlineNotice kind="info">
                    이 반복 그룹은 전체 내용을 선택해야 해요. 선택한 쪽의 모든
                    반복 항목을 사용하며, 다른 필드의 내용은 유지합니다.
                  </InlineNotice>
                )}
              {change.status === "blocked" && (
                <InlineNotice kind="warning">
                  현재 구조에 적용할 수 없는 내용이에요. 저장하지 않은 입력은
                  유지됩니다. 먼저 해당 정의나 이전 버전을 복원한 뒤 다시 확인해
                  주세요.
                </InlineNotice>
              )}
              <ExternalMediaPreviewContext.Provider value={false}>
                <MediaActiveContext.Provider value={false}>
                  <div className="recovery-compare-columns">
                    <section>
                      <h4>현재 내용</h4>
                      <ValuePreview
                        value={change.current}
                        field={field}
                        names={names}
                        property={property}
                        reference={
                          reference
                            ? { ...reference, preview: true, open: () => {} }
                            : undefined
                        }
                      />
                    </section>
                    <section>
                      <h4>작성 중 내용</h4>
                      <ValuePreview
                        value={change.preserved}
                        field={field}
                        names={names}
                        property={property}
                        reference={
                          reference
                            ? { ...reference, preview: true, open: () => {} }
                            : undefined
                        }
                      />
                    </section>
                  </div>
                </MediaActiveContext.Provider>
              </ExternalMediaPreviewContext.Provider>
              {change.status === "conflict" && (
                <RadioGroup
                  name={`draft-choice-${index}`}
                  aria-label={label}
                  value={choices[change.id] ?? ""}
                  disabled={busy}
                  onChange={(_, data) =>
                    setChoices((previous) => ({
                      ...previous,
                      [change.id]: data.value,
                    }))
                  }
                >
                  <Radio value="current" label="현재 내용 사용" />
                  <Radio value="draft" label="작성 중 내용 사용" />
                </RadioGroup>
              )}
            </section>
          );
        })}
      <Button
        type="button"
        appearance="primary"
        disabled={busy || conflicts.some((change) => !choices[change.id])}
        onClick={() =>
          void apply(
            conflicts
              .filter((change) => choices[change.id] === "draft")
              .map((change) => change.id),
          )
        }
      >
        선택한 내용으로 편집 이어가기
      </Button>
      {cancel && (
        <Button type="button" disabled={busy} onClick={() => void cancel()}>
          비교 취소
        </Button>
      )}
    </section>
  );
}

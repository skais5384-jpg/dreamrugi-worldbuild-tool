import { Label } from "@fluentui/react-components";
import type { ReactNode } from "react";

/**
 * 반복 속성의 이름과 내용을 같은 행으로 묶는다.
 * 표 역할을 만들지 않아 기존 form의 읽기·Tab 순서와 label 연결을 보존한다.
 */
export function PropertyRow({
  label,
  htmlFor,
  children,
  complex = false,
  block = false,
  before,
  presentation = "",
}: {
  label: ReactNode;
  htmlFor?: string;
  children: ReactNode;
  /** 여러 입력과 동작이 함께 있는 내용은 아주 좁을 때만 위쪽 라벨로 전환한다. */
  complex?: boolean;
  /** Narrative, groups and references use the full value width without nested labels. */
  block?: boolean;
  before?: ReactNode;
  presentation?: string;
}) {
  return (
    <>
      {before}
      <div
        className={
          "property-row " +
          presentation +
          (complex ? " property-row-complex" : "") +
          (block ? " property-row-block" : "")
        }
      >
        {htmlFor ? (
          <Label className="property-label" htmlFor={htmlFor}>
            {label}
          </Label>
        ) : (
          <div className="property-label">{label}</div>
        )}
        <div className="property-content">{children}</div>
      </div>
    </>
  );
}

export function blockField(kind?: string) {
  return [
    "Text",
    "RichText",
    "Group",
    "Relation",
    "DocumentLink",
    "Image",
    "Attachment",
  ].includes(kind ?? "");
}

import { useState } from "react";
import { act, cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { render } from "../test/render";
import { SectionEditor, repairSectionAnchors } from "./SectionEditor";
import { presentationClass, SectionTitles } from "./Presentation";
import type { TemplateBody } from "../bridge/workspace";
import type { Template } from "../bridge/types";
import { text } from "../strings";
afterEach(cleanup);
it("adds, renames, reorders and removes a heading without editing field values", async () => {
  const field = {
    id: "f",
    label: "수치",
    configuration: { kind: "number" as const },
    required: false,
    presentation: { intent: "keep" as const },
    default: { intent: "keep" as const },
    archived: false,
  };
  let latest: TemplateBody = {
    name: "T",
    presentation: { intent: "keep" },
    fields: [field],
    composing: false,
  };
  function Harness() {
    const [body, update] = useState(latest);
    return (
      <SectionEditor
        owner="o"
        generation="1"
        body={body}
        disabled={false}
        change={(f) =>
          update((old) => {
            latest = repairSectionAnchors(old, f(old));
            return latest;
          })
        }
      />
    );
  }
  const view = render(<Harness />);
  const details = view.container.querySelector("details")!;
  await act(async () => {
    details.open = true;
    fireEvent(details, new Event("toggle"));
  });
  fireEvent.click(screen.getByRole("button", { name: text("section.add") }));
  fireEvent.change(
    screen.getByRole("textbox", { name: text("section.title") }),
    { target: { value: "상황" } },
  );
  fireEvent.keyDown(
    screen.getByRole("button", { name: text("section.manage") + ": 상황" }),
    { key: "ArrowUp", altKey: true },
  );
  expect(latest.sections).toHaveLength(1);
  expect(latest.sections![0].beforeField).toBe("f");
  expect(latest.fields).toEqual([field]);
  fireEvent.click(screen.getByRole("button", { name: text("section.remove") }));
  expect(latest.sections).toEqual([]);
  expect(latest.fields).toEqual([field]);
});
it("ignores legacy presentation tokens and renders only title content with no value control", () => {
  expect(presentationClass("emphasis", null)).toBe("presentation-standard");
  expect(presentationClass("emphasis", "quiet")).toBe("presentation-standard");
  expect(presentationClass("emphasis", "url(https://invalid.test)")).toBe(
    "presentation-standard",
  );
  const template = {
    sections: [{ id: "s", title: "상황", beforeField: "f" }],
  } as Template;
  render(<SectionTitles template={template} before="f" />);
  expect(screen.getByRole("heading", { name: "상황" })).toBeInTheDocument();
  expect(screen.queryByRole("textbox")).toBeNull();
});

import { useState } from "react";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogSurface,
} from "@fluentui/react-components";
import { render } from "../test/render";
import { ActivityLog, useActivityLog } from "../app/ActivityLog";
import {
  FloatingMessage,
  FloatingNotice,
  NoticeEventDetail,
  useNoticeHistory,
} from "./FloatingNotice";
import { Button, Fieldset } from "./Controls";

describe("shared lower-left notifications", () => {
  it("all notices and retained actions are inside their activity table rows", async () => {
    const retry = vi.fn();
    function Fixture() {
      const [open, setOpen] = useState(false);
      const log = useActivityLog(false);
      return (
        <>
          <Button onClick={() => setOpen(true)}>show records</Button>
          <FloatingNotice intent="warning">
            settings need checking{" "}
            <Button onClick={retry}>retry setting</Button>
          </FloatingNotice>
          {open && (
            <ActivityLog
              events={log.events}
              droppedEvents={0}
              close={() => setOpen(false)}
            />
          )}
        </>
      );
    }
    render(<Fixture />);
    const alert = await screen.findByRole("alert");
    expect(within(alert).getAllByRole("button")).toHaveLength(2);
    for (const name of ["자세히", "알림 닫기"]) {
      const command = within(alert).getByRole("button", { name });
      expect(command).toContainHTML("<svg");
      expect(command).toHaveTextContent("");
    }
    expect(
      within(alert).queryByRole("button", { name: "retry setting" }),
    ).toBeNull();
    fireEvent.click(within(alert).getByRole("button", { name: "알림 닫기" }));
    fireEvent.click(screen.getByRole("button", { name: "show records" }));
    const dialog = await screen.findByRole("dialog");
    const summary = await within(dialog).findByText("settings need checking", {
      selector: "summary",
    });
    expect(summary.closest("table")).not.toBeNull();
    fireEvent.click(summary);
    const button = within(dialog).getByRole("button", {
      name: "retry setting",
    });
    expect(button.closest("td")).not.toBeNull();
    fireEvent.click(button);
    expect(retry).toHaveBeenCalledOnce();
    expect(within(dialog).queryByRole("region", { name: "알림" })).toBeNull();
    expect(
      within(dialog)
        .getAllByText("settings need checking")
        .every((node) => node.closest("table")),
    ).toBe(true);
  });
  it("moving an action retains the owner's disabled controls", async () => {
    const action = vi.fn();
    function Fixture() {
      const history = useNoticeHistory();
      return (
        <>
          <Fieldset disabled>
            <FloatingNotice intent="warning">
              waiting <Button onClick={action}>protected action</Button>
            </FloatingNotice>
          </Fieldset>
          <section aria-label="row">
            {history[0] && <NoticeEventDetail event={history[0]} />}
          </section>
        </>
      );
    }
    render(<Fixture />);
    const stack = await screen.findByRole("region", { name: "알림" });
    const button = await within(
      screen.getByRole("region", { name: "row" }),
    ).findByRole("button", {
      name: "protected action",
    });
    expect(button).toBeDisabled();
    fireEvent.click(button);
    expect(action).not.toHaveBeenCalled();
    expect(
      within(stack).getByRole("button", { name: "알림 닫기" }),
    ).toBeEnabled();
  });
  it("dismissal retains history and live retry actions; changed notices appear again", async () => {
    const retry = vi.fn();
    function Fixture() {
      const [step, setStep] = useState(0);
      const history = useNoticeHistory();
      return (
        <>
          <Button onClick={() => setStep(step + 1)}>change</Button>
          <output aria-label="history count">{history.length}</output>
          <FloatingNotice intent="error">
            failed {step}
            <Button disabled={step > 0} onClick={retry}>
              retry
            </Button>
          </FloatingNotice>
          <section aria-label="record actions">
            {history.length > 0 && (
              <NoticeEventDetail event={history[history.length - 1]} />
            )}
          </section>
        </>
      );
    }
    render(<Fixture />);
    const stack = await screen.findByRole("region", { name: "알림" });
    const toast = await within(stack).findByRole("alert");
    expect(within(toast).queryByRole("button", { name: "retry" })).toBeNull();
    fireEvent.click(
      await within(
        screen.getByRole("region", { name: "record actions" }),
      ).findByRole("button", { name: "retry" }),
    );
    expect(retry).toHaveBeenCalledOnce();
    const openLog = vi.fn();
    window.addEventListener("open-activity-log", openLog, { once: true });
    fireEvent.click(within(toast).getByRole("button", { name: "자세히" }));
    expect(openLog).toHaveBeenCalledOnce();
    fireEvent.click(within(toast).getByRole("button", { name: "알림 닫기" }));
    await waitFor(() => expect(within(stack).queryByRole("alert")).toBeNull());
    expect(screen.getByLabelText("history count")).toHaveTextContent("1");
    expect(
      within(screen.getByRole("region", { name: "record actions" })).getByRole(
        "button",
        { name: "retry" },
      ),
    ).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "change" }));
    const next = await within(stack).findByRole("alert");
    expect(next).toHaveTextContent("failed 1");
    expect(within(next).queryByRole("button", { name: "retry" })).toBeNull();
    expect(screen.getByLabelText("history count")).toHaveTextContent("2");
    expect(
      within(screen.getByRole("region", { name: "record actions" })).getByRole(
        "button",
        { name: "retry" },
      ),
    ).toBeDisabled();
  });

  it("a dialog owns the shared stack, and existing operation buttons retain their behavior", async () => {
    const cancel = vi.fn();
    function Fixture() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <Button onClick={() => setOpen(true)}>open</Button>
          <FloatingMessage>
            <div className="feedback-toast" role="status">
              working <Button onClick={cancel}>cancel</Button>
            </div>
          </FloatingMessage>
          <Dialog open={open} onOpenChange={(_, data) => setOpen(data.open)}>
            <DialogSurface>
              <DialogBody>
                <DialogContent>
                  <FloatingNotice intent="warning">
                    check saved data
                  </FloatingNotice>
                  <Button onClick={() => setOpen(false)}>close dialog</Button>
                </DialogContent>
              </DialogBody>
            </DialogSurface>
          </Dialog>
        </>
      );
    }
    const view = render(<Fixture />);
    fireEvent.click(await screen.findByRole("button", { name: "open" }));
    const dialog = await screen.findByRole("dialog");
    const stack = await within(dialog).findByRole("region", { name: "알림" });
    expect(within(stack).getByText("check saved data")).toBeVisible();
    fireEvent.click(within(stack).getByRole("button", { name: "cancel" }));
    expect(cancel).toHaveBeenCalledOnce();
    expect(view.container.querySelector(".fui-MessageBar")).toBeNull();
    fireEvent.click(
      within(dialog).getByRole("button", { name: "close dialog" }),
    );
    await waitFor(() =>
      expect(
        screen
          .getByRole("region", { name: "알림" })
          .closest(".fui-DialogSurface"),
      ).toBeNull(),
    );
    expect(screen.getByRole("button", { name: "cancel" })).toBeEnabled();
  });
});

it("keeps diagnostics out of the ordinary screen and preserves them after the notice owner closes", async () => {
  function Fixture() {
    const [active, setActive] = useState(true);
    const [open, setOpen] = useState(false);
    const history = useNoticeHistory();
    return (
      <>
        <Button
          onClick={() => {
            setActive(false);
            setOpen(true);
          }}
        >
          show retained record
        </Button>
        {active && (
          <FloatingNotice
            intent="warning"
            recordDetail={<p>synthetic_backup_diagnostic</p>}
          >
            백업 확인이 필요합니다.
          </FloatingNotice>
        )}
        {open && (
          <section aria-label="retained record">
            {history.map((event) => (
              <NoticeEventDetail key={event.sessionId} event={event} />
            ))}
          </section>
        )}
      </>
    );
  }
  render(<Fixture />);
  await screen.findByRole("alert");
  expect(screen.queryByText("synthetic_backup_diagnostic")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "show retained record" }));
  await within(
    screen.getByRole("region", { name: "retained record" }),
  ).findByText("synthetic_backup_diagnostic");
});

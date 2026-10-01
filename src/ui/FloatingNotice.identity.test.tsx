import { useRef, useState } from "react";
import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { render } from "../test/render";
import type { ActivityEvent } from "../app/ActivityLog";
import {
  FloatingNotice,
  NoticeEventDetail,
  useNoticeHistory,
} from "./FloatingNotice";
import { Button } from "./Controls";

it("FIX002 regression: a historical row must not acquire a new operation when the same message recurs", async () => {
  const first = vi.fn();
  const second = vi.fn();
  function Fixture() {
    const [step, setStep] = useState(0);
    const history = useNoticeHistory();
    const old = useRef<ActivityEvent | null>(null);
    if (!old.current && history.length) old.current = history[0];
    return (
      <>
        <Button onClick={() => setStep(step + 1)}>advance</Button>
        <output aria-label="count">{history.length}</output>
        <FloatingNotice intent="warning" eventId={step}>
          {step === 1 ? "Different failure" : "Settings could not be saved"}
          <Button onClick={step === 0 ? first : second}>retry</Button>
        </FloatingNotice>
        <section aria-label="current">
          {history[history.length - 1] && (
            <NoticeEventDetail event={history[history.length - 1]!} />
          )}
        </section>
        <section aria-label="old event">
          {old.current && <NoticeEventDetail event={old.current} />}
        </section>
      </>
    );
  }
  render(<Fixture />);
  await waitFor(() =>
    expect(screen.getByLabelText("count")).toHaveTextContent("1"),
  );
  const old = screen.getByRole("region", { name: "old event" });
  fireEvent.click(within(old).getByRole("button", { name: "retry" }));
  expect(first).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole("button", { name: "advance" }));
  await waitFor(() =>
    expect(screen.getByLabelText("count")).toHaveTextContent("2"),
  );
  expect(within(old).queryByRole("button", { name: "retry" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "advance" }));
  await waitFor(() =>
    expect(screen.getByLabelText("count")).toHaveTextContent("3"),
  );
  const resurrected = within(old).queryByRole("button", { name: "retry" });
  if (resurrected) fireEvent.click(resurrected);
  expect(second).not.toHaveBeenCalled();
  expect(resurrected).toBeNull();
  fireEvent.click(
    within(screen.getByRole("region", { name: "current" })).getByRole(
      "button",
      {
        name: "retry",
      },
    ),
  );
  expect(second).toHaveBeenCalledOnce();
});

it("same event rerenders and disabled updates retain one history row and dismissal access", async () => {
  const action = vi.fn();
  function Fixture() {
    const [busy, setBusy] = useState(true);
    const [tick, setTick] = useState(0);
    const history = useNoticeHistory();
    return (
      <>
        <Button onClick={() => setBusy(!busy)}>toggle busy</Button>
        <Button onClick={() => setTick(tick + 1)}>rerender</Button>
        <output aria-label="count">{history.length}</output>
        <FloatingNotice eventId="request-one" intent="warning">
          Saved settings need checking
          <Button disabled={busy} onClick={() => action(tick)}>
            retry
          </Button>
        </FloatingNotice>
        <section aria-label="row">
          {history[0] && <NoticeEventDetail event={history[0]} />}
        </section>
      </>
    );
  }
  render(<Fixture />);
  const row = screen.getByRole("region", { name: "row" });
  const button = await within(row).findByRole("button", { name: "retry" });
  expect(button).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "rerender" }));
  fireEvent.click(screen.getByRole("button", { name: "toggle busy" }));
  await waitFor(() =>
    expect(within(row).getByRole("button", { name: "retry" })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole("button", { name: "알림 닫기" }));
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  expect(screen.getByLabelText("count")).toHaveTextContent("1");
  fireEvent.click(within(row).getByRole("button", { name: "retry" }));
  expect(action).toHaveBeenCalledExactlyOnceWith(1);
});

it("project/session replacement and a stale DOM click cannot redirect the previous action", async () => {
  const oldAction = vi.fn();
  const newAction = vi.fn();
  function Fixture() {
    const [scope, setScope] = useState("project-A:session-1");
    const history = useNoticeHistory();
    return (
      <>
        <Button onClick={() => setScope("project-B:session-2")}>
          replace session
        </Button>
        <output aria-label="count">{history.length}</output>
        <FloatingNotice eventId="request-one" scope={scope} intent="warning">
          Same failure{" "}
          <Button
            onClick={scope.startsWith("project-A") ? oldAction : newAction}
          >
            retry
          </Button>
        </FloatingNotice>
        <section aria-label="current">
          {history[history.length - 1] && (
            <NoticeEventDetail event={history[history.length - 1]!} />
          )}
        </section>
        <section aria-label="old">
          {history[0] && <NoticeEventDetail event={history[0]} />}
        </section>
      </>
    );
  }
  render(<Fixture />);
  const old = screen.getByRole("region", { name: "old" });
  const retained = await within(old).findByRole("button", { name: "retry" });
  fireEvent.click(screen.getByRole("button", { name: "replace session" }));
  fireEvent.click(retained);
  expect(oldAction).not.toHaveBeenCalled();
  expect(newAction).not.toHaveBeenCalled();
  await waitFor(() =>
    expect(screen.getByLabelText("count")).toHaveTextContent("2"),
  );
  expect(within(old).queryByRole("button", { name: "retry" })).toBeNull();
  fireEvent.click(
    within(screen.getByRole("region", { name: "current" })).getByRole(
      "button",
      {
        name: "retry",
      },
    ),
  );
  expect(newAction).toHaveBeenCalledOnce();
});

it("click-time owner invalidation rejects a displayed action before the next render", async () => {
  const action = vi.fn();
  let valid = true;
  function Fixture() {
    const history = useNoticeHistory();
    return (
      <>
        <FloatingNotice eventId="request-one" isCurrent={() => valid}>
          Check this owner <Button onClick={action}>retry</Button>
        </FloatingNotice>
        <section aria-label="current">
          {history[0] && <NoticeEventDetail event={history[0]} />}
        </section>
      </>
    );
  }
  render(<Fixture />);
  const button = await screen.findByRole("button", { name: "retry" });
  valid = false;
  fireEvent.click(button);
  expect(action).not.toHaveBeenCalled();
});

it("FIX002 regression: equal message with a new owner must not redirect the old row action", async () => {
  const otherOwner = vi.fn();
  function Fixture() {
    const [owner, setOwner] = useState("A");
    const history = useNoticeHistory();
    return (
      <>
        <Button onClick={() => setOwner("B")}>change owner</Button>
        <FloatingNotice intent="warning" eventId={owner}>
          Settings could not be saved
          <Button onClick={() => otherOwner(owner)}>retry {owner}</Button>
        </FloatingNotice>
        <section aria-label="current">
          {history[history.length - 1] && (
            <NoticeEventDetail event={history[history.length - 1]!} />
          )}
        </section>
        <section aria-label="old row">
          {history[0] && <NoticeEventDetail event={history[0]} />}
        </section>
      </>
    );
  }
  render(<Fixture />);
  const old = screen.getByRole("region", { name: "old row" });
  await within(old).findByRole("button", { name: "retry A" });
  fireEvent.click(screen.getByRole("button", { name: "change owner" }));
  await within(screen.getByRole("region", { name: "current" })).findByRole(
    "button",
    { name: "retry B" },
  );
  const redirected = within(old).queryByRole("button", { name: "retry B" });
  if (redirected) fireEvent.click(redirected);
  expect(redirected).toBeNull();
  expect(otherOwner).not.toHaveBeenCalledWith("B");
});

import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Checkbox,
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
} from "@fluentui/react-components";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text } from "../strings";
import { useYoutubeConsent, youtubeConsentStore } from "./youtubeConsent";
import koPolicy from "../privacy/PRIVACY-POLICY.ko.md?raw";
import enPolicy from "../privacy/PRIVACY-POLICY.en.md?raw";
import "./PrivacyDialog.css";
import { PrivacyPolicyText } from "./PrivacyPolicyText";

export function PrivacyDialog({
  open,
  close,
}: {
  open: boolean;
  close: () => void;
}) {
  const consent = useYoutubeConsent();
  const [policy, setPolicy] = useState<"ko" | "en" | null>(null);
  const [accepted, setAccepted] = useState(false);
  const [allowed, setAllowed] = useState(false);
  const [failed, setFailed] = useState(false);
  const store = youtubeConsentStore();
  const dismiss = () => {
    // Closing the first notice is a remembered refusal, not consent.
    if (store.snapshot().choice === "unset") store.choose(false);
    setPolicy(null);
    setAccepted(false);
    setAllowed(false);
    setFailed(false);
    close();
  };
  const choose = (yes: boolean) => {
    if (yes && (!accepted || !allowed)) return;
    if (store.choose(yes)) dismiss();
  };
  return (
    <Dialog
      open={open}
      onOpenChange={(_, data) => {
        if (!data.open) dismiss();
      }}
    >
      <DialogSurface className="privacy-surface">
        <DialogBody>
          <DialogTitle>
            {text(policy ? "privacy.policy" : "privacy.title")}
          </DialogTitle>
          <DialogContent className="privacy-content">
            {policy ? (
              <>
                <div className="privacy-links">
                  <Button onClick={() => setPolicy("ko")}>
                    {text("privacy.languageKo")}
                  </Button>
                  <Button onClick={() => setPolicy("en")}>
                    {text("privacy.languageEn")}
                  </Button>
                </div>
                <PrivacyPolicyText
                  content={policy === "ko" ? koPolicy : enPolicy}
                  failed={() => setFailed(true)}
                />
              </>
            ) : (
              <>
                <p>{text("privacy.explanation")}</p>
                <div className="privacy-links">
                  <Button onClick={() => setPolicy("ko")}>
                    {text("privacy.policy")}
                  </Button>
                  {(["google_privacy", "youtube_terms"] as const).map(
                    (target) => (
                      <Button
                        key={target}
                        onClick={() => {
                          setFailed(false);
                          void invoke("about_open_link", { target }).catch(() =>
                            setFailed(true),
                          );
                        }}
                      >
                        {text(
                          target === "google_privacy"
                            ? "privacy.google"
                            : "privacy.youtubeTerms",
                        )}
                      </Button>
                    ),
                  )}
                </div>
                <p>
                  {text(
                    consent.choice === "allowed"
                      ? "privacy.allowed"
                      : "privacy.denied",
                  )}
                </p>
                {consent.choice !== "allowed" && (
                  <>
                    <Checkbox
                      checked={accepted}
                      label={text("privacy.acceptPolicy")}
                      onChange={(_, d) => setAccepted(d.checked === true)}
                    />
                    <Checkbox
                      checked={allowed}
                      label={text("privacy.allowYoutube")}
                      onChange={(_, d) => setAllowed(d.checked === true)}
                    />
                  </>
                )}
                {consent.storageFailed && (
                  <InlineNotice kind="warning">
                    {text("privacy.storageFailed")}
                  </InlineNotice>
                )}
              </>
            )}
            {failed && (
              <InlineNotice kind="warning">
                {text("privacy.linkFailed")}
              </InlineNotice>
            )}
          </DialogContent>
          <DialogActions>
            {policy ? (
              <Button onClick={() => setPolicy(null)}>
                {text("privacy.close")}
              </Button>
            ) : (
              <>
                {consent.choice === "allowed" ? (
                  <Button onClick={() => choose(false)}>
                    {text("privacy.revoke")}
                  </Button>
                ) : (
                  <>
                    <Button onClick={() => choose(false)}>
                      {text("privacy.deny")}
                    </Button>
                    <Button
                      appearance="primary"
                      disabled={!accepted || !allowed}
                      onClick={() => choose(true)}
                    >
                      {text("privacy.allow")}
                    </Button>
                  </>
                )}
                <Button onClick={dismiss}>{text("privacy.close")}</Button>
              </>
            )}
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

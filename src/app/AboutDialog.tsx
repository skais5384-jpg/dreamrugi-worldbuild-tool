import { invoke } from "@tauri-apps/api/core";
import {
  Dialog,
  DialogActions,
  DialogBody,
  DialogContent,
  DialogSurface,
  DialogTitle,
} from "@fluentui/react-components";
import { useEffect, useRef, useState } from "react";
import { Button } from "../ui/Controls";
import { InlineNotice } from "../ui/InlineNotice";
import { text, type StringKey } from "../strings";
import spellNotice from "../../src-tauri/spellcheck/NOTICE.md?raw";
import dictionaryLicense from "../../src-tauri/spellcheck/resources/dictionary-LICENSE.md?raw";
import dictionaryGpl from "../../src-tauri/spellcheck/resources/dictionary-GPL-3.txt?raw";
import hunspellMpl from "../../src-tauri/about-notices/hunspell-MPL-1.1.txt?raw";
import hunspellGpl from "../../src-tauri/about-notices/hunspell-GPL-2.txt?raw";
import hunspellLgpl from "../../src-tauri/about-notices/hunspell-LGPL-2.1.txt?raw";
import lexicalLicense from "../../src-tauri/about-notices/lexical.txt?raw";
import fluentLicense from "../../src-tauri/about-notices/fluent-ui.txt?raw";
import obsidianLicense from "../../src-tauri/about-notices/obsidian-border.txt?raw";
import fontLicense from "../../src-tauri/about-notices/noto-cjk.txt?raw";
import appLicense from "../../LICENSE?raw";
import "./AboutDialog.css";

const notices: { id: string; label: StringKey; content: string }[] = [
  { id: "app", label: "about.notice.app", content: appLicense },
  { id: "spell", label: "about.notice.spell", content: spellNotice },
  {
    id: "dictionary",
    label: "about.notice.dictionary",
    content: dictionaryLicense,
  },
  {
    id: "dictionary-gpl",
    label: "about.notice.dictionaryGpl",
    content: dictionaryGpl,
  },
  {
    id: "hunspell-mpl",
    label: "about.notice.hunspellMpl",
    content: hunspellMpl,
  },
  {
    id: "hunspell-gpl",
    label: "about.notice.hunspellGpl",
    content: hunspellGpl,
  },
  {
    id: "hunspell-lgpl",
    label: "about.notice.hunspellLgpl",
    content: hunspellLgpl,
  },
  { id: "lexical", label: "about.notice.lexical", content: lexicalLicense },
  { id: "fluent", label: "about.notice.fluent", content: fluentLicense },
  { id: "obsidian", label: "about.notice.obsidian", content: obsidianLicense },
  { id: "font", label: "about.notice.font", content: fontLicense },
];

export function AboutDialog({
  open,
  close,
}: {
  open: boolean;
  close: () => void;
}) {
  const [version, setVersion] = useState<string | null>(null);
  const [channel, setChannel] = useState<string | null>(null);
  const [versionFailed, setVersionFailed] = useState(false);
  const [noticeId, setNoticeId] = useState<string | null>(null);
  const [linkError, setLinkError] = useState(false);
  const noticeBack = useRef<HTMLButtonElement>(null);
  const notice = notices.find((item) => item.id === noticeId);
  useEffect(() => {
    if (!open) return;
    let current = true;
    void invoke<string>("about_version")
      .then((value) => {
        if (current) {
          setVersion(value);
          setVersionFailed(false);
        }
      })
      .catch(() => {
        if (current) {
          setVersion(null);
          setVersionFailed(true);
        }
      });
    void invoke<string>("about_channel")
      .then((value) => {
        if (current) setChannel(value);
      })
      .catch(() => {
        if (current) setChannel(null);
      });
    return () => {
      current = false;
    };
  }, [open]);
  useEffect(() => {
    if (open && notice) noticeBack.current?.focus();
  }, [open, notice]);
  const dismiss = () => {
    setNoticeId(null);
    setLinkError(false);
    close();
  };
  const link = (target: "blog" | "email" | "source") => {
    setLinkError(false);
    void invoke("about_open_link", { target }).catch(() => setLinkError(true));
  };
  return (
    <Dialog
      open={open}
      onOpenChange={(_, data) => {
        if (!data.open) dismiss();
      }}
    >
      <DialogSurface className="about-surface">
        <DialogBody>
          <DialogTitle>
            {notice ? text(notice.label) : text("about.title")}
          </DialogTitle>
          <DialogContent className="about-content">
            {notice ? (
              <pre className="about-notice-text" tabIndex={0}>
                {notice.content}
              </pre>
            ) : (
              <>
                <div className="about-logos">
                  <img
                    src="/brand/worldbuild-tool-logo.png"
                    alt={text("about.worldbuildLogo")}
                  />
                  <img
                    src="/brand/dreamrugi-logo.png"
                    alt={text("about.dreamrugiLogo")}
                  />
                </div>
                <h2>{text("about.name")}</h2>
                <p className="about-description">{text("about.description")}</p>
                <div className="about-facts">
                  <div>
                    <strong>{text("about.version")}</strong>
                    <span>
                      {versionFailed
                        ? text("about.versionUnavailable")
                        : (version ?? text("about.versionLoading"))}
                    </span>
                  </div>
                  <div>
                    <strong>{text("about.licenses")}</strong>
                    <span>{text("about.licenseSummary")}</span>
                  </div>
                  <div>
                    <strong>{text("about.channel")}</strong>
                    <span>{channel ?? text("about.channelUnavailable")}</span>
                  </div>
                </div>
                <p className="about-license-note">{text("about.licenseOwn")}</p>
                <div
                  className="about-notice-list"
                  aria-label={text("about.licenses")}
                >
                  {notices.map((item) => (
                    <Button
                      key={item.id}
                      type="button"
                      size="small"
                      appearance="subtle"
                      onClick={() => setNoticeId(item.id)}
                    >
                      {text(item.label)}
                    </Button>
                  ))}
                </div>
                <p className="about-source-note">{text("about.sources")}</p>
                <Button
                  type="button"
                  appearance="subtle"
                  onClick={() => link("source")}
                >
                  {text("about.sourceRepo")}
                </Button>
                <div className="about-contact">
                  <h3>{text("about.contact")}</h3>
                  <Button
                    type="button"
                    appearance="subtle"
                    onClick={() => link("blog")}
                  >
                    {text("about.blog")}
                  </Button>
                  <Button
                    type="button"
                    appearance="subtle"
                    onClick={() => link("email")}
                  >
                    {text("about.email")}
                  </Button>
                </div>
                {linkError && (
                  <InlineNotice kind="warning">
                    {text("about.linkFailed")}
                  </InlineNotice>
                )}
              </>
            )}
          </DialogContent>
          <DialogActions>
            {notice && (
              <Button
                ref={noticeBack}
                type="button"
                onClick={() => setNoticeId(null)}
              >
                {text("about.back")}
              </Button>
            )}
            <Button
              type="button"
              appearance={notice ? "secondary" : "primary"}
              onClick={dismiss}
            >
              {text("about.close")}
            </Button>
          </DialogActions>
        </DialogBody>
      </DialogSurface>
    </Dialog>
  );
}

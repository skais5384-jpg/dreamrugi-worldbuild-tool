import { Fragment, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";

const targets: Record<string, string> = {
  "mailto:skais5384@naver.com": "email",
  "https://policies.google.com/privacy": "google_privacy",
  "https://www.youtube.com/t/terms": "youtube_terms",
  "https://www.microsoft.com/en-us/privacy/privacystatement":
    "microsoft_privacy",
  "https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/data-privacy":
    "webview_privacy",
  "https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement":
    "github_privacy",
  "https://policy.naver.com/policy/privacy.html": "naver_privacy",
};
// Only bundled prose, simple formatting, and allowlisted links. No HTML or media.
function inline(value: string, failed: () => void): ReactNode {
  const parts: ReactNode[] = [];
  const pattern = /\[([^\]]+)\]\(([^)]+)\)|\*\*([^*]+)\*\*/g;
  let start = 0;
  for (const match of value.matchAll(pattern)) {
    parts.push(value.slice(start, match.index));
    const target = targets[match[2]];
    parts.push(
      <Fragment key={match.index}>
        {match[3] ? (
          <strong>{match[3]}</strong>
        ) : target ? (
          <a
            href={match[2]}
            onClick={(event) => {
              event.preventDefault();
              void invoke("about_open_link", { target }).catch(failed);
            }}
          >
            {match[1]}
          </a>
        ) : (
          match[1]
        )}
      </Fragment>,
    );
    start = match.index! + match[0].length;
  }
  parts.push(value.slice(start));
  return parts;
}
export function PrivacyPolicyText({
  content,
  failed,
}: {
  content: string;
  failed: () => void;
}) {
  return (
    <article className="privacy-policy" tabIndex={0}>
      {content
        .trim()
        .split(/\r?\n\s*\r?\n/)
        .map((block, i) => {
          if (block.startsWith("|")) {
            const rows = block.split(/\r?\n/).map((line) =>
              line
                .split("|")
                .slice(1, -1)
                .map((cell) => cell.trim()),
            );
            return (
              <div className="privacy-table" key={i}>
                <table>
                  <thead>
                    <tr>
                      {rows[0].map((cell, c) => (
                        <th key={c}>{inline(cell, failed)}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {rows.slice(2).map((row, r) => (
                      <tr key={r}>
                        {row.map((cell, c) => (
                          <td key={c}>{inline(cell, failed)}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            );
          }
          if (block.startsWith("## ")) return <h2 key={i}>{block.slice(3)}</h2>;
          if (block.startsWith("# ")) return <h1 key={i}>{block.slice(2)}</h1>;
          if (block.startsWith("- "))
            return (
              <ul key={i}>
                {block.split(/\r?\n/).map((line, n) => (
                  <li key={n}>{inline(line.replace(/^- /, ""), failed)}</li>
                ))}
              </ul>
            );
          return <p key={i}>{inline(block, failed)}</p>;
        })}
    </article>
  );
}

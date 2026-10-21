import { useEffect, useState } from "react";
import { useT } from "../i18n/useT";
import { errorCode } from "../lib/deviceErrors";
import { getAccessIssues, onAccessIssuesChanged, saveUdevRule } from "../lib/ipc";
import type { AccessIssue } from "../lib/types";
import { Button } from "../ui";

const DEFAULT_RULE_PATH = "~/Downloads/70-myhub.rules";

function installCommand(rulePath: string): string {
  return `sudo cp ${rulePath} /etc/udev/rules.d/ && sudo udevadm control --reload-rules && sudo udevadm trigger`;
}

/** Linux only in practice: the list is empty elsewhere. Explains the udev rule when a device cannot be opened. */
export function AccessBanner() {
  const t = useT();
  const [issues, setIssues] = useState<AccessIssue[]>([]);
  const [note, setNote] = useState<string | null>(null);
  const [rulePath, setRulePath] = useState(DEFAULT_RULE_PATH);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    getAccessIssues().then(
      (list) => {
        if (!cancelled) setIssues(list);
      },
      () => {},
    );
    onAccessIssuesChanged(setIssues).then(
      (fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      },
      () => {},
    );
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  if (issues.length === 0) return null;

  const save = () => {
    saveUdevRule().then(
      (path) => {
        // Single-quoted for the shell: the real path may hold spaces.
        setRulePath(`'${path.replaceAll("'", "'\\''")}'`);
        setNote(t("access.saved", { path }));
      },
      (error: unknown) => setNote(t("access.saveError", { message: errorCode(error) })),
    );
  };

  return (
    <div role="status" className="mb-6 flex flex-col gap-3 rounded-lg border border-border bg-surface px-4 py-3 text-sm text-text">
      <span>{t("access.message")}</span>
      <code className="select-text break-all rounded bg-surface-2 px-3 py-2 font-mono text-xs">{installCommand(rulePath)}</code>
      <div className="flex items-center gap-3">
        <Button variant="primary" size="sm" onClick={save}>
          {t("access.save")}
        </Button>
        {note ? <span className="min-w-0 break-words">{note}</span> : null}
      </div>
    </div>
  );
}

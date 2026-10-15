import type { MessageKey } from "../i18n/en";

/** Pulls the contract error string out of whatever a rejected invoke produced. */
export function errorCode(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "";
}

const headsetErrorKeys = {
  writesDisabled: "hs.err.writesDisabled",
  disconnected: "hs.err.disconnected",
  outOfRange: "hs.err.outOfRange",
  notSupported: "hs.err.notSupported",
  verifyFailed: "hs.err.verifyFailed",
  confirmRequired: "hs.err.confirmRequired",
  alertMissing: "hs.err.alertMissing",
  alertRefused: "hs.err.alertRefused",
  "deviceError:timeout": "hs.err.timeout",
} as const satisfies Record<string, MessageKey>;

/** Maps a headset write error code to a localized message key plus placeholder values. */
export function headsetErrorMessage(error: unknown): { key: MessageKey; vars?: Record<string, string> } {
  const code = errorCode(error);
  const known = (headsetErrorKeys as Record<string, MessageKey | undefined>)[code];
  if (known) return { key: known };
  if (code.startsWith("deviceError:")) return { key: "hs.err.device", vars: { code: code.slice("deviceError:".length) } };
  return { key: "hs.err.unknown" };
}

/** Maps a set_dpi / set_report_rate error code to a localized message key plus placeholder values. */
export function deviceErrorMessage(error: unknown): { key: MessageKey; vars?: Record<string, string> } {
  const code = errorCode(error);
  switch (code) {
    case "writesDisabled":
      return { key: "err.writesDisabled" };
    case "notHostMode":
      return { key: "err.notHostMode" };
    case "disconnected":
      return { key: "err.disconnected" };
    case "outOfRange":
      return { key: "err.outOfRange" };
    case "verifyFailed":
      return { key: "err.verifyFailed" };
    case "profileNotWritable":
      return { key: "err.profileNotWritable" };
    case "notOnboardMode":
      return { key: "err.notOnboardMode" };
    case "backupFailed":
      return { key: "err.backupFailed" };
  }
  if (code.startsWith("savedReselectFailed")) {
    const sector = /^0x([0-9a-f]+)$/i.exec(code.slice("savedReselectFailed:".length));
    const value = sector ? parseInt(sector[1] ?? "", 16) : NaN;
    if (value >= 0x0001 && value <= 0x0005) return { key: "err.savedReselectFailed", vars: { n: String(value) } };
    if (value >= 0x0101 && value <= 0x0105) return { key: "err.savedReselectFailed", vars: { n: `ROM ${value - 0x0100}` } };
    return { key: "err.savedReselectUnknown" };
  }
  if (code.startsWith("flashWriteFailed:")) {
    switch (code.slice("flashWriteFailed:".length)) {
      case "restored":
        return { key: "err.flash.restored" };
      case "restoreFailed":
        return { key: "err.flash.restoreFailed" };
      case "unknown":
        return { key: "err.flash.unknown" };
      case "notNeeded":
        return { key: "err.flash.notNeeded" };
    }
    return { key: "err.flash.unknown" };
  }
  if (code.startsWith("deviceError:")) return { key: "err.device", vars: { code: code.slice("deviceError:".length) } };
  return { key: "err.unknown" };
}

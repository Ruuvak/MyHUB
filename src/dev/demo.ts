/** Dev preview only: maps a `?demo=` value to fixture devices. Loaded through a dynamic import in lib/ipc.ts. */
import type { DeviceDetail, DeviceSummary } from "../lib/types";
import { demoDetail as g502xDetail } from "./g502x.fixture";
import { buildK2heDetail } from "./k2he.fixture";
import { buildSonyDetail } from "./sony.fixture";

/** Fixture devices for a `?demo=` value, the mouse first. Empty for any other value. */
export function demoDetails(demo: string | null): DeviceDetail[] {
  const list: Array<DeviceDetail | null> = [];
  if (demo === "all") list.push(g502xDetail("g502x"), buildSonyDetail(true), buildK2heDetail(false));
  else if (demo === "sony") list.push(buildSonyDetail(true));
  else if (demo === "k2he") list.push(buildK2heDetail(false));
  else if (demo === "k2he-busy") list.push(buildK2heDetail(true));
  else if (demo === "sony-off") list.push(buildSonyDetail(false));
  else list.push(g502xDetail(demo));
  return list.filter((d): d is DeviceDetail => d !== null);
}

export function demoSummaries(demo: string | null): DeviceSummary[] {
  return demoDetails(demo).map((d) => ({
    id: d.id,
    name: d.name,
    kind: d.kind,
    connection: d.connection,
    batteryPercent: d.batteryPercent,
  }));
}

import { Bluetooth } from "@phosphor-icons/react";
import { WirelessReceiverIcon } from "../../ui/icons";

/** Small icon for the connection type: receiver for LIGHTSPEED, Bluetooth for Bluetooth, nothing else. */
export function TransportIcon({ transport, size = 16 }: { transport: string; size?: number }) {
  if (transport === "LIGHTSPEED") return <WirelessReceiverIcon size={size} className="shrink-0" />;
  if (transport === "Bluetooth") return <Bluetooth size={size} weight="regular" aria-hidden="true" className="shrink-0" />;
  return null;
}

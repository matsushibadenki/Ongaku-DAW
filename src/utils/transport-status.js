const stateLabels = {
  accepted: "transportAccepted", pending: "transportPending", waiting: "transportWaiting",
  applied: "transportApplied", cancelPending: "transportCancelPending", canceled: "transportCanceled",
  failed: "transportFailed", capacityExceeded: "transportCapacity",
};
const reasonLabels = { explicit: "transportExplicit", superseded: "transportSuperseded", stopped: "transportStopped" };

/** Describe one retained result; never attribute an older ID's state to the latest acceptance. */
export function transportStatusHelp(protocol, t) {
  const latest = protocol?.latest;
  if (!Number.isSafeInteger(latest?.commandId) || latest.commandId <= 0) return "";
  const result = latest.result?.commandId === latest.commandId ? latest.result : null;
  const label = latest.historyLost ? t.transportHistoryLost
    : latest.updating || !result ? t.transportUpdating : t[stateLabels[result.state]] ?? t.transportUpdating;
  const validResult = !latest.historyLost && !latest.updating ? result : null;
  const reason = validResult ? t[reasonLabels[validResult.cancelReason]] : "";
  const retained = validResult?.effectApplied && ["canceled", "cancelPending"].includes(validResult.state) ? t.transportNoRollback : "";
  return [`${t.transportCommand} #${latest.commandId}: ${label}`, reason, retained].filter(Boolean).join(" · ");
}

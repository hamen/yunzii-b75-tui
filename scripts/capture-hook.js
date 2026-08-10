// Yunzii B75 Pro Max WebHID capture hook.
// Inject into https://yunzii-game.com/#/screen with the device already connected
// (navigator.hid permission already granted). Run as a top-level script (NOT
// wrapped in an async IIFE — an unresolved Promise return serializes as `{}`
// in some injection tools). Read results from window.__hidLog afterward.
//
// Captures: outbound sendReport + sendFeatureReport, inbound inputreport
// events + receiveFeatureReport results. Safe to re-run; installs hooks once.

window.__hidLog = window.__hidLog || [];

function toHex(buf) {
  const arr = buf instanceof ArrayBuffer ? new Uint8Array(buf) : new Uint8Array(buf.buffer || buf);
  return Array.from(arr).map(b => b.toString(16).padStart(2, '0')).join(' ');
}

if (!window.__hidHookInstalled) {
  window.__hidHookInstalled = true;

  const origSendReport = HIDDevice.prototype.sendReport;
  HIDDevice.prototype.sendReport = function (reportId, data) {
    window.__hidLog.push({ t: Date.now(), dir: 'out', method: 'sendReport', reportId, hex: toHex(data) });
    return origSendReport.call(this, reportId, data);
  };

  const origSendFeatureReport = HIDDevice.prototype.sendFeatureReport;
  HIDDevice.prototype.sendFeatureReport = function (reportId, data) {
    window.__hidLog.push({ t: Date.now(), dir: 'out', method: 'sendFeatureReport', reportId, hex: toHex(data) });
    return origSendFeatureReport.call(this, reportId, data);
  };

  const origReceiveFeatureReport = HIDDevice.prototype.receiveFeatureReport;
  HIDDevice.prototype.receiveFeatureReport = function (reportId) {
    return origReceiveFeatureReport.call(this, reportId).then((dv) => {
      window.__hidLog.push({ t: Date.now(), dir: 'in', method: 'receiveFeatureReport', reportId, hex: toHex(dv.buffer) });
      return dv;
    });
  };
}

// Top-level await: must be run without an async-IIFE wrapper.
const devices = await navigator.hid.getDevices();
devices.forEach((d) => {
  if (!d.__hooked) {
    d.__hooked = true;
    d.addEventListener('inputreport', (e) => {
      window.__hidLog.push({ t: Date.now(), dir: 'in', method: 'inputreport', reportId: e.reportId, hex: toHex(e.data.buffer) });
    });
  }
});

({ installed: true, deviceCount: devices.length, logLen: window.__hidLog.length });

// Interface enumeration/identity (run separately, once, to confirm which of the
// N navigator.hid.getDevices() entries is the vendor config channel):
//
// const devices = await navigator.hid.getDevices();
// devices.map((d, i) => ({ index: i, opened: d.opened, collections: d.collections.map(c => ({
//   usagePage: c.usagePage.toString(16), usage: c.usage.toString(16),
//   inputReports: c.inputReports.map(r => r.reportId),
//   outputReports: c.outputReports.map(r => r.reportId),
// })) }));
//
// On this machine, 2026-08-10: 4 HID interfaces under VID 0x28E9 / PID 0x31C8.
// Index 2 has usagePage 0xFF60 / usage 0x61 (the standard QMK/VIA "Raw HID"
// usage), report ID 0, 64-byte input AND output reports, and is the one the
// site has already opened ("opened": true) when connected. This is almost
// certainly the vendor config channel used for screen settings.

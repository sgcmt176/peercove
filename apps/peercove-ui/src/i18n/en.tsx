// English catalog. Translated from ja.tsx (the source of truth for keys and
// structure). Key names, function signatures and JSX shape must stay
// identical to ja.tsx — only the values are translated. See index.ts for how
// this catalog is registered and selected.

import { ReactNode } from "react";
import type { Strings } from "./index";

export const en: Strings = {
  // General-purpose labels/buttons used in multiple places
  common: {
    close: "Close",
    cancel: "Cancel",
    save: "Save",
    saving: "Saving…",
    retry: "Retry",
    loading: "Loading…",
    running: "Running…",
    useAnotherConfig: "Use a different config file",
    configFilter: "PeerCove config",
    configLabel: "Config",
    configFile: "Config file",
    virtualIp: "Virtual IP",
    delete: "Delete",
    add: "Add",
  },

  // Tunnel state (ipc.ts stateLabel / badges)
  state: {
    idle: "Idle",
    hosting: "Running as host",
    joined: "Joined as member",
    runningCount: (n: number) => (n === 1 ? "Running" : `${n} networks running`),
    daemonDisconnected: "Daemon not connected",
    connectingDaemon: "Connecting to daemon…",
  },

  // Number formatting (ipc.ts format helpers)
  format: {
    none: "None",
    secondsAgo: (n: number) => (n === 1 ? "1 second ago" : `${n} seconds ago`),
    minutesAgo: (n: number) => (n === 1 ? "1 minute ago" : `${n} minutes ago`),
    hoursAgo: (n: number) => (n === 1 ? "1 hour ago" : `${n} hours ago`),
  },

  header: {
    myDisplayName: (name: string) => `Display name: ${name}`,
    settings: "Settings",
    settingsUnavailable: "No config file yet (start hosting or join a network)",
    prefs: "App settings",
    logs: "Daemon logs",
    // Appearance theme toggle button (M3-6). Light ⇄ dark
    theme: (theme: "light" | "dark") =>
      theme === "light" ? "Appearance: Light (click for dark)" : "Appearance: Dark (click for light)",
  },

  // Left sidebar (M3-15. Shell redesign)
  sidebar: {
    networks: "Networks",
    memos: "Notes",
    sharedMemos: "Shared",
    members: "Members",
    chat: "Chat",
    stats: "Quality",
    inbox: "Inbox",
    dns: "DNS",
    subnets: "Subnets",
    acl: "Access control",
    groups: "Groups",
    diagnostics: "Diagnostics",
    settings: "Settings",
    connected: "Connected",
    disconnected: "Disconnected",
    theme: (theme: "light" | "dark") =>
      theme === "light" ? "Appearance: Light (click for dark)" : "Appearance: Dark (click for light)",
    logs: "Logs",
    version: (v: string) => `v${v}`,
  },

  diagnostics: {
    title: "Connection diagnostics",
    lead: "Checks your settings and current state without changing anything. Nothing is sent to an external diagnostic server.",
    rerun: "Re-run",
    running: "Running diagnostics…",
    noIssues: "No problems found within what could be checked.",
    passed: (count: number) => `Healthy checks (${count})`,
    unknownAction: "Not enough information for a detailed verdict.",
    passAction: "No problems were found.",
    evidence: {
      peercoveAclManaged: "Managed by PeerCove (current user + SYSTEM)",
      modeBitsVerified: "Unix permissions verified",
    },
    overall: {
      healthy: "Healthy",
      attention: "Needs attention",
      problem: "Problem found",
    },
    status: {
      pass: "OK",
      warning: "Warning",
      fail: "Failed",
      unknown: "Unknown",
    },
    check: {
      "app.ipc_compatible": {
        summary: "UI and daemon IPC are compatible",
        action: "If they mismatch, update the UI and daemon to the same version.",
      },
      "app.version_known": {
        summary: "Daemon version information",
        action: "If unknown, update the daemon. The connection itself can continue.",
      },
      "app.peer_compatibility": {
        summary: "Version compatibility of connected members",
        action: "Unknown members may be on an older version. Update if some features don't work.",
      },
      "config.valid": {
        summary: "Config file loads and validates",
        action: "If it fails, fix the config error shown.",
      },
      "permissions.secret_files": {
        summary: "Secret files exist with correct permissions",
        action: "Restore any missing files and make sure other users can't read them.",
      },
      "tunnel.running": {
        summary: "Tunnel running state",
        action: "If stopped, connect from the network screen.",
      },
      "tunnel.interface_ready": {
        summary: "Virtual interface",
        action: "If the name or address is missing, reconnect and check for administrator rights.",
      },
      "tunnel.handshake": {
        summary: "Handshake with peer",
        action: "If there's no response, check the endpoint, firewall, and whether the host is running.",
      },
      "internet.reachability_evidence": {
        summary: "Evidence of internet reachability",
        action: "No external server is used, so this can't be judged without a real handshake.",
      },
      "dns.zone_available": {
        summary: "Built-in DNS zone information",
        action: "If the member list hasn't arrived, restore the connection to the host first.",
      },
      "tunnel.host_removed_member": {
        summary: "This member has been removed by the host",
        action: "Disconnect, then get a new invite from the host administrator.",
      },
      "tunnel.acl": {
        summary: "Blocked by access control",
        action: "Check whether this is intentional, or review the host's access control settings.",
      },
      // Shared memo (M5 F-3, ADR-0049). This check is skipped entirely if there's no DB (shared memos unused)
      "memo.db_status": {
        summary: "Shared notes database health",
        action: "Size, count, or WAL size is approaching the limit. Consider raising the limit or cleaning up unneeded notes.",
      },
      "memo.cache_status": {
        summary: "Shared notes cache size",
        action: "This is cleaned up automatically on reconnect to the host. If it doesn't improve, consider deleting the cache file.",
      },
    },
  },

  quality: {
    title: "Connection quality",
    description: "Stores RTT, packet loss, and route history for 7 days, only on this device.",
    peer: "Member",
    period: "Time range",
    summary: "Summary",
    latestRtt: "Latest RTT",
    averageRtt: "Average RTT",
    p95Rtt: "P95 RTT",
    loss: "Loss rate",
    lastConnected: "Last connected",
    rttChart: "RTT over time",
    lossChart: "Packet loss",
    route: "Route",
    table: "Raw data",
    time: "Time",
    state: "Connection state",
    jitter: "Jitter",
    transfer: "Transfer",
    direct: "Direct",
    relay: "Via host",
    trying: "Establishing",
    connected: "Connected",
    disconnected: "No control connection",
    unmeasured: "Not measured",
    // Explain why it can't be measured (M7 J-3 follow-up, per requester). RTT is
    // measured over the control channel between the host and each member, so
    // members can't measure each other directly (communication itself still works via the host).
    unmeasuredReason:
      "RTT and packet loss are measured over the control connection with the host, so values between members can't be measured (not applicable). This device can only measure quality with the host. Communication with the other member itself still works via the host.",
    loading: "Loading quality history…",
    empty: "No quality data yet. It takes a few seconds after connecting for the first measurement.",
    noRtt: "No RTT measurements in this period.",
    noLoss: "No pings in this period to compute a loss rate from.",
    gaps: "Periods without a control connection are shown as gaps, not connected lines.",
    lossNote: "A control connection outage is not counted as 100% loss.",
    rttAria: "Line chart of average RTT per hour. The numbers are also available in the table below.",
    lossAria: "Bar chart of packet loss rate per hour. The numbers are also available in the table below.",
    switches: (count: number) => `${count} route switch${count === 1 ? "" : "es"}`,
    corrupt: (count: number) =>
      count === 1
        ? "Skipped 1 unreadable history line safely."
        : `Skipped ${count} unreadable history lines safely.`,
  },

  footer: (
    <>
      wintun.dll © WireGuard LLC — bundled unmodified under the Prebuilt Binaries
      License (see <span className="mono">wintun-LICENSE.txt</span> in the install
      directory).
    </>
  ),

  // App-wide settings (M3-13e). Applies only to this machine (localStorage)
  prefs: {
    title: "App settings",
    language: "Language",
    languageJa: "日本語",
    languageEn: "English",
    languageHint: "The display language for this device. Defaults to the OS language setting.",
    languageReloadConfirm: "This switches the language and reloads the app. Continue?",
    notifications: "Show OS notifications",
    notificationsHint:
      "Notifications for members joining/disconnecting, new chat messages, and completed file receives. Turning this off doesn't change unread badges or the inbox tab display.",
    linkPreview: "Show link previews for chat URLs",
    linkPreviewHint:
      "When a message containing a URL is shown, this device fetches information (title, image) from that page. Turn this off if you don't want your IP address exposed to the other site.",
    qualityAlerts: "Notify when connection quality drops",
    qualityAlertsHint:
      "Notifies when the loss rate stays above the threshold for 3 minutes, and when the route switches from direct to via the host. Off by default.",
    qualityLossThreshold: "Loss rate threshold",
    note: "This setting is saved only on this machine (applies immediately).",
  },

  update: {
    title: "Updates and version",
    enabled: "Check for new versions automatically",
    enabledHint: "Checks GitHub Releases once a day for the latest version only. No settings or network information is sent.",
    uiVersion: "App",
    daemonVersion: "Daemon",
    latestVersion: "Latest",
    unknown: "Unknown",
    checkNow: "Check now",
    checking: "Checking…",
    disabledHint: "Automatic update checks are off. Enable the switch above to check.",
    current: "You're using the latest version.",
    available: (version: string) => `v${version} is available.`,
    openRelease: "Open release page",
    sidebar: (version: string) => `v${version} · Update available`,
    memberVersion: (version: string) => `PeerCove v${version}`,
    memberVersionUnknown: "PeerCove version unknown",
    memberPlatformTitle: "This device's OS (reported at connection time)",
  },

  daemonOutdated: {
    title: "Daemon update required",
    body: "The running daemon is an old version, so status can't be shown correctly (the tunnel may show \"stopped\" even while running). Please replace the service:",
    windows:
      "Administrator PowerShell: Stop-Service peercove-daemon → with the new peercove.exe, daemon service-uninstall → daemon service-install",
    linux:
      "sudo systemctl stop peercove-daemon → replace the binary → sudo systemctl start peercove-daemon",
  },

  daemonUnreachable: {
    title: "Can't connect to the daemon",
    body: "Operating the tunnel requires a daemon with administrator rights. Run the following in a terminal:",
    command: "peercove daemon run",
    platforms: "Use an administrator terminal on Windows, or sudo on Linux.",
    details: "Details",
  },

  // Network list (M3-0c)
  networks: {
    listHead: "Networks",
    empty: (
      <>
        No networks yet. Either <strong>become the host (center)</strong> and create
        one, or use an invite token to <strong>join an existing network</strong>.
      </>
    ),
    running: "Running",
    stopped: "Stopped",
    roleHost: "Host",
    roleMember: "Member",
    membersOnline: (n: number) => `${n} online`,
    open: "Open",
    connect: "Connect",
    connecting: "Connecting…",
    disconnecting: "Disconnecting…",
    removedBadge: "Removed",
    external: "(running with a config outside the list)",
    back: "← Back to list",
    settings: "Settings",
    delete: "Delete",
    deleteTitle: "Delete network",
    deleteConfirm: "Delete",
    deleteMessage: (name: string): ReactNode => (
      <>
        <p>
          This deletes the config for network <strong>{name}</strong>.
        </p>
        <p className="muted">
          The key files are deleted with it, so this can't be undone. Rejoining
          requires a new invite token. If you were hosting, members will no
          longer be able to connect.
        </p>
      </>
    ),
    addHost: "Start hosting",
    addJoin: "Join with an invite token",
    addClose: "Close",
    nameLabel: "Network name",
    nameHint: "Alphanumeric and hyphens only. Used for the folder name and future DNS address (〜.peercove.internal).",
    namePlaceholder: "home",
    create: "Create and start hosting",
    creating: "Creating…",
  },

  start: {
    intro: (
      <>
        Choose whether to <strong>become the center (host)</strong> of this network
        or <strong>join</strong> an existing one. Either way you can pick "start
        new" or "resume from a saved config".
      </>
    ),
    locateError: "Couldn't locate the config",
    host: {
      title: "Start hosting",
      lead: "Your PC becomes the center of the network. You can invite participants.",
      stateLabel: "State",
      stateExisting: "Use an existing network",
      stateNew: "Create new (auto-generates keys and address)",
      upnp: (
        <>
          Automatically open a router port (UPnP)
          <small className="muted"> — needed when inviting people on a different network</small>
        </>
      ),
      creating: "Creating network…",
      starting: "Starting tunnel…",
      start: "Start",
      createAndStart: "Create and start",
      note: "Operating the tunnel requires a daemon with administrator rights (this app itself runs with normal rights).",
    },
    join: {
      title: "Join",
      lead: "Join a network someone else is hosting, as a member.",
      savedHead: "Reconnect with a saved config",
      reconnect: "Reconnect to the previous network",
      starting: "Starting tunnel…",
      creating: "Creating join config…",
      toggleClose: "Close joining with a new invite",
      toggleOpen: "Join a different network (invite token)",
      newHead: "Join with a new invite token",
      tokenHint: (
        <>
          Paste the invite token you received from the host (a string starting
          with <code>pcv1.</code>).
        </>
      ),
      tokenPlaceholder: "pcv1.…",
      overwrite: "Overwrite the existing join config",
      submit: "Join",
    },
  },

  tunnel: {
    removedTitle: "Removed by the host",
    removedBody:
      "You have been removed from this network. Communication is already blocked. Click \"Disconnect\", then join again with a new invite token if needed.",
    rejectedTitle: "Connection rejected",
    rejectedBadge: "Join rejected",
    rejectedAction: "This join config can't reconnect. Click \"Disconnect\", then join again with a new invite token issued by the host.",
    disconnectConfirm: "Disconnect",
    disconnect: "Disconnect",
    back: "Back to list",
    connected: "Connected",
    // Network display name change (ADR-0058, host only). The DNS name doesn't change
    renameNetwork: "Rename network display name",
    renameHint: "Only the display name changes. The DNS name stays the same.",
    configFileLabel: "Config file",
    membersHead: (n: number) => `Members (${n})`,
    // Stat cards right under the detail header (M3-15)
    overview: {
      virtualIp: "Virtual IP",
      online: "Online",
      onlineCount: (n: number) => `${n}`,
      rate: "Transfer rate",
    },
    // Member table headers (M3-15)
    table: {
      role: "Role",
      dnsName: "DNS name",
      virtualIp: "Virtual IP",
      rate: "Transfer rate",
      rtt: "Latency",
      actions: "Actions",
      copyIp: "Copy virtual IP",
      chat: "Open chat",
    },
    // DNS service card (M3-15, using the URL from ADR-0023)
    service: {
      head: "DNS services",
      hint: "Custom DNS records with a scheme configured can be opened here with one click (visible to all members).",
      copyUrl: "Copy URL",
      copied: "Copied",
      openTitle: "Open in default browser",
    },
    // Network detail tabs (M3-6)
    tabs: {
      chat: "Chat",
      stats: "Quality",
      inbox: "Inbox",
    },
    invite: "Invite member",
    ledgerPending: "The member list hasn't been received yet (this can take a few seconds right after connecting).",
    removeNotice: (name: string) => `Removed ${name}. They'll drop off the tunnel in about 10 seconds.`,
    peers: {
      head: "Peer stats",
      publicKey: "Public key",
      endpoint: "Endpoint",
      lastHandshake: "Last handshake",
      rtt: "RTT",
      rate: "Rate",
      rx: "Received",
      tx: "Sent",
      notConnected: "(not connected)",
      rttTitle: "Round-trip time measured over the in-tunnel control channel (line shows the last ~90 seconds)",
      rateTitle: "Combined receive + send rate (line shows the last ~90 seconds)",
      empty: "No stats yet (this can take a few seconds right after connecting).",
    },
    remove: {
      title: "Remove member",
      confirm: "Remove",
      message: (who: string): ReactNode => (
        <>
          <p>
            This removes <strong>{who}</strong> from the network.
          </p>
          <p className="muted">
            They'll be notified and drop off the tunnel in about 10 seconds. The
            invite token you gave them will also stop working. To let them
            rejoin, issue a new invite.
          </p>
        </>
      ),
    },
    // Bulk member removal (M6 H-8d). Same convention as the checkbox selection for file sending
    bulkRemove: {
      open: "Bulk remove",
      title: "Bulk remove members",
      selectAll: "Select all",
      // Off by default. Online members can't be removed unless this is checked (per requester)
      allowOnline: "Also remove online members",
      onlineBlocked: (names: string[]) =>
        `Online members can't be removed: ${names.join(", ")}. Check "Also remove online members" to include them.`,
      confirmMessage: (n: number): ReactNode => (
        <>
          <p>
            This removes the selected <strong>{n} member{n === 1 ? "" : "s"}</strong> from
            the network.
          </p>
          <p className="muted">
            They'll be notified and drop off the tunnel in about 10 seconds. The
            invite tokens you gave them will also stop working. To let them
            rejoin, issue new invites.
          </p>
        </>
      ),
      submit: (n: number) => (n === 0 ? "Remove" : `Remove ${n} member${n === 1 ? "" : "s"}`),
      result: (ok: number, failed: number) =>
        failed > 0 ? `Removed ${ok} (${failed} failed).` : `Removed ${ok}.`,
    },
    member: {
      online: "Online",
      offline: "Offline",
      noName: "(no name)",
      rttTitle: "Round-trip time within the tunnel",
      rateTitle: "Transfer rate with this member (receive + send, last ~90 seconds)",
      rename: "Rename display name",
      // Display name change (ADR-0027, M3-19). The member can change it from their own row
      displayRenamed: "Display name changed. Takes a few seconds to reach everyone.",
      // DNS name split from display name (ADR-0021, M3-14a)
      editDns: "Change DNS name (alphanumeric and hyphens only, independent of display name)",
      dnsRenamed: "DNS name changed. Takes a few seconds to reach everyone.",
      remove: "Remove",
      self: "You",
      selfTitle: "This is your own machine",
      // Route between members (M3-4, ADR-0013)
      route: {
        direct: "Direct",
        trying: "Establishing…",
        relay: "Relayed",
        aclRelay: "Relayed by ACL",
        aclTitle: "Fixed to route via the host so fine-grained ACLs apply reliably. The string next to it is the rule ID.",
        title:
          "The route used to reach this member. \"Direct\" means a direct connection, \"Relayed\" means via the host, and \"Establishing…\" means a direct connection is being attempted.",
      },
      // ACL (M3-10, ADR-0018)
      blocked: "Blocked",
      blockedTitle: "Communication with this member is blocked by the host's access control",
      inviteStatus: {
        legacy: "Existing",
        pending: "Unused",
        joined: "Joined",
        awaiting_approval: "Awaiting approval",
        expired: "Expired",
        clock_invalid: "Clock error",
      },
      approve: "Approve",
      approved: "Approved the joining device. Isolation will be lifted within seconds.",
      inviteExpires: (value: string) => `Invite expires: ${value}`,
      // Member detail page (ADR-0048)
      detail: "Details",
      detailTitle: (name: string) => `${name} details`,
      detailName: "Display name",
      detailRole: "Role",
      detailDns: "DNS name",
      detailOs: "OS",
      detailVersion: "App",
      detailKey: "Public key",
      detailState: "State",
      detailRoute: "Route",
      detailInvite: "Invite status",
      detailSubnets: "Published subnets",
      invitedBy: "Invited by",
      invitedByHost: "Host",
      // Designating devices allowed to issue invites (ADR-0048)
      canInviteLabel: "Allow this device to issue member invites",
      // Shown as a tooltip (label title). Keep it to 1-2 concise sentences
      canInviteHint: "Allows this device to invite new members (issuing is recorded in the host's log and posted to the group chat)",
      // Admin members are shown checked and non-interactive (ADR-0059 J-2 expansion, per requester)
      canInviteAdminHint: "Admins can always issue invites (this can't be turned off individually)",
      canInviteUpdated: "Changed invite-issuing permission. Takes a few seconds to apply.",
      // Delegated admin (ADR-0059, M7 J-1b → expanded to near-host permissions in J-2)
      adminBadge: "Admin",
      adminBadgeTitle: "Delegated admin (can operate almost the same as the host, except granting/revoking admin)",
      adminLabel: "Delegate admin",
      adminHint:
        "Delegates admin to this device (can manage members, network settings, shared notes permissions, and DNS settings at host level, except granting/revoking admin)",
      adminUpdated: "Changed delegated admin. Takes a few seconds to apply.",
    },
    // Explanation of direct communication (M3-4). Explicitly mentions sharing your external IP (ADR-0013 condition 3)
    directNote:
      "Members communicate directly when possible (faster, no load on the host's connection). When this happens, your public IP address is shared with the other member on the same network. If you don't want this, turn off direct communication in \"Settings\".",
  },

  // Chat (M3-13b/c, ADR-0016)
  chat: {
    all: "All",
    allNote: "Reaches everyone on the network (only members online at send time)",
    empty: "No messages yet.",
    noMessages: "No messages",
    pin: "Pin (always show at top)",
    unpin: "Unpin",
    // Unread mention badge (conversation list, M6 H-7b)
    mentionBadgeTitle: "There's an unread mention of you",
    mute: "Mute notifications",
    unmute: "Unmute",
    moveUp: "Move up",
    moveDown: "Move down",
    searchPlaceholder: "Search messages",
    searchEmpty: "No results",
    previewSelf: "You: ",
    placeholder: "Type a message (Enter to send, Shift+Enter for a new line, files can be pasted)",
    offline: "Can't send to offline members",
    blocked: "Can't send to this member due to the host's access control",
    left: "This member is no longer on the network (history only)",
    send: "Send",
    failed: "Send failed",
    // Send queue (E-E 3)
    sendingState: "Sending…",
    retrying: "Not sent (will retry automatically)",
    resend: "Resend",
    cancelSend: "Cancel",
    // Groups (M3-13c)
    groupCreate: "Create group",
    groupTitle: "New group",
    groupManage: "Manage",
    groupNameLabel: "Group name",
    groupNamePlaceholder: "e.g. Dev team",
    groupMembersLabel: "Choose members",
    groupAddLabel: "Add member",
    groupRemoveLabel: "Remove member",
    groupMembersHead: "Members",
    groupNoCandidates: "No members available to add.",
    groupCount: (n: number) => `${n}`,
    groupNote: "Group information is shared directly between members. It's delivered automatically to offline members once they come back online.",
    create: "Create",
    save: "Save",
    leave: "Leave",
    leaveTitle: "Leave group",
    leaveConfirm: (name: string) => `Leave "${name}"? Your history for it will stay on this device.`,
    leftGroup: "You've left this group (history only)",
    unknownGroup: "Group (syncing)",
    groupPending: "Waiting to receive group info (arrives automatically once a member comes online)",
    // In-chat file sending + drag & drop (M3-13d)
    attach: "Send a file",
    filePreview: (name: string) => `📎 ${name}`,
    fileFailed: "Transfer failed",
    fileStarted: (count: number) =>
      count === 1 ? "Started sending the file." : `Started sending ${count} files.`,
    dropTitle: "Send file",
    dropMessage: (dest: string) => `Send to "${dest}"?`,
    dropHint: (dest: string) => `Drop to send to "${dest}"`,
    // Text file preview (M3-13e)
    textOpen: "Click to show the full text",
    textTruncated: "The file is large, so only the beginning is shown.",
  },

  // Shared object reference `@memo:id` card (M5 F-5 Stage 4, ADR-0052 decision 1)
  sharedRef: {
    loading: "Loading…",
    inaccessible: (noun: string) => `Inaccessible ${noun}`,
    nounMemo: "note",
    nounSchedule: "event",
    nounSheet: "sheet",
  },

  // File send/receive box (M3-9b, ADR-0015)
  transfer: {
    head: "Transfers",
    // Direction shown at the start of each row
    direction: (d: "send" | "recv") => (d === "send" ? "Sent" : "Received"),
    done: "Done",
    failed: (reason: string) => `Failed: ${reason}`,
    progress: (transferred: string, size: string) => `${transferred} / ${size}`,
    // File send dialog (M3-13e: recipients chosen via checkboxes)
    sendButton: "📤 Send file",
    dialogTitle: "Send file",
    fileLabel: "File to send",
    pick: "Choose file",
    noFile: "(none selected)",
    recipientsLabel: "Recipients (multiple allowed)",
    noCandidates: "No members available to send to.",
    dialogNote: "Can't send to offline members. Progress is shown per recipient in the transfer list on the inbox tab.",
    sendTo: (n: number) => (n === 0 ? "Send" : `Send to ${n} member${n === 1 ? "" : "s"}`),
    startedMany: (n: number) =>
      n === 1
        ? "Started sending (progress is shown on this tab)."
        : `Started sending to ${n} members (progress is shown on this tab).`,
  },
  inbox: {
    head: "Inbox",
    note: "Received files are added here automatically. Use \"Save\" to move them wherever you like.",
    empty: "No files received.",
    from: (who: string) => `From ${who}`,
    save: "Save",
    delete: "Delete",
    savedTo: (path: string) => `Saved to: ${path}`,
    deleted: (name: string) => `Deleted ${name}.`,
  },

  // Subnet sharing (M3-7b, ADR-0014 → made a full page in M3-16)
  subnet: {
    pageTitle: "Subnets (share a LAN behind a member)",
    intro:
      "Makes a LAN behind a member (home NAS, printer, another PC, etc.) reachable by everyone on the network. Only the host can configure this, and changes reach everyone in about 10 seconds.",
    memberLabel: (name: string) => `LAN published by ${name}`,
    placeholder: "192.168.10.0/24",
    hint: "Specify as an IP range (CIDR), e.g. 192.168.10.0/24. Multiple ranges can be space-separated; save with it empty to unpublish.",
    empty: "No members available to publish a LAN yet (once a member other than the host joins, you can configure it here).",
    note: "Only Linux members can publish (act as a gateway for) a LAN — Windows can only be a destination. Once published, that LAN is reachable from every member, and devices on the LAN will see the traffic as coming from that member's device (NAT).",
    save: "Save",
    saved: "Saved. Reaches everyone in about 10 seconds.",
    badgeTitle: "The subnet of a LAN this member is publishing (reachable from all members)",
  },

  // Access control ACL (M3-10, ADR-0018)
  acl: {
    button: "🚦 Access control",
    title: "Access control between members",
    intro:
      "Between two people set to \"Block\", all communication — including chat and file sending — is blocked (communication with the host can't be blocked). Changes reach everyone in about 5 seconds.",
    needTwo: "You can configure this once there are 2 or more members.",
    block: "Block",
    blockedTag: "Blocked",
    introV2: "The first matching rule from the top applies. Direction refers to the direction that starts a new connection; responses on an allowed reverse direction still pass. You can target members, groups, or subnets, and specify protocol and ports. Changes take about 5 seconds to apply.",
    defaultAction: "Traffic matching no rule",
    allow: "Allow",
    deny: "Deny",
    rules: "Rules",
    noRules: "No individual rules. The default action is used.",
    order: "Order",
    action: "Action",
    source: "Source",
    destination: "Destination",
    protocol: "Protocol",
    ports: "Destination ports",
    state: "State",
    actions: "Actions",
    allPorts: "All",
    enabled: "Enabled",
    disabled: "Disabled",
    moveUp: "Move up one",
    moveDown: "Move down one",
    addRule: "Add rule",
    relayWarning: "Member-to-member traffic covered by this rule switches to routing via the host, to guarantee the rule is evaluated.",
    any: "Any",
    member: "Member",
    group: "Group",
    subnet: "Subnet",
    service: "DNS service",
    targetRequired: "Choose a source and a destination.",
    groups: "Member groups",
    groupName: "New group name",
    addGroup: "Add group",
    people: (n: number): string => `${n} ${n === 1 ? "member" : "members"}`,
    groupsMovedNotice:
      "Create, rename, and edit group membership from the \"Groups\" screen in the side menu. Here you can only pick groups as a rule's source or destination.",
  },

  // Permission groups (ADR-0060). A shared "who's included" unit used by
  // access control, shared memos, and more. Its own sidebar screen (PermGroupView).
  permGroups: {
    title: "Groups",
    intro:
      "Groups of members used for access control and shared memo permissions. The host, and members delegated as admin, can create, rename, edit membership, and delete. Changes reach everyone via ledger distribution (this can take a few seconds).",
    viewOnlyNote: "View only here (editing is done by the host or an admin).",
    empty: "There are no groups yet.",
    membersLabel: "Members",
    unresolved: " (unresolved)",
    create: "Create group",
    createTitle: "Create group",
    nameLabel: "Group name",
    namePlaceholder: "e.g. Servers",
    membersPick: "Pick members",
    hostIncludeNote:
      "Including the host has no effect on access control (traffic via the host is outside the scope of access control).",
    rename: "Rename",
    renameTitle: "Rename group",
    editMembers: "Edit members",
    editMembersTitle: "Edit members",
    deleteTitle: "Delete group",
    deleteConfirm: (name: string) =>
      `This deletes "${name}". Any access control rules that use this group will be deleted along with it. Continue?`,
    createDisabledHint: "Enter a group name and pick at least one member.",
  },

  // DNS management screen (M3-1c, extended with custom records in ADR-0022/0023)
  dns: {
    title: "DNS",
    button: "DNS",
    intro: (
      <>
        Members can be reached at <code>dnsname.networkname.peercove.internal</code>.
        The DNS name can be changed from the ✎ in the member list.
      </>
    ),
    autoHead: "Automatic records (members)",
    autoEmpty: "The member list hasn't been received yet.",
    customHead: "Custom records",
    customEmpty: "No custom records.",
    customNote:
      "The host, and members with delegated admin, can add and remove these. They're distributed to all members in about 10 seconds. " +
      "Choosing a machine on the left side makes it a subdomain of that machine, and it follows automatically when the machine is renamed.",
    customNoteMember: "Custom records are managed by the host (view only here).",
    // For members with delegated admin (ADR-0059 decision 3, J-1c).
    customNoteAdmin: "As an admin, you can add, remove, and configure custom records on the host's behalf.",
    adminApplyNotice: "Change submitted. Takes a few seconds to apply.",
    adminMissingId: "This record is in an old format and can't be operated on (opening the DNS screen once on the host resolves this).",
    // Domain name input (ADR-0024)
    domainLabel: "Domain name",
    prefixPlaceholder: "* / web / api, etc. (optional)",
    baseFree: "Free-form",
    baseFreePlaceholder: "app",
    baseMember: (name: string) => `${name} (machine)`,
    wildcardHint:
      "The left side can be empty, or dot-separated like web or api.v2. Prefixing with * makes that one label a wildcard (e.g. *.app). Choosing a machine from the suggestions makes it a subdomain of that machine.",
    baseIsMachine: "This becomes a subdomain of this machine (follows automatically when the machine is renamed). Enter the left side (e.g. * or web).",
    previewLabel: "Name that will be registered",
    // Forward target (ADR-0024)
    forwardLabel: "Forward target (what this domain points to)",
    forwardIp: "Specify an IP address",
    forwardCname: "Specify a domain (CNAME)",
    forwardMember: (name: string) => `Machine: ${name}`,
    cnamePlaceholder: "docs.example.com",
    cnameHint: "Makes this an alias (CNAME) of another domain. External domains (e.g. example.com) can also be specified. Resolvers that look this up will be redirected to the target.",
    cnameTag: "→ Other domain",
    ipPlaceholder: "10.68.1.50",
    schemeLabel: "Scheme (optional)",
    schemePlaceholder: "http",
    portLabel: "Port (optional)",
    portPlaceholder: "8080",
    serviceHint: "If you specify a scheme, a URL is built and shown to all members. If only a port is specified, it's shown as dnsname:port.",
    targetOf: (name: string) => `→ ${name}`,
    openTitle: "Open in default browser",
    brokenRef: "No target",
    add: "Add",
    adding: "Adding…",
    remove: "Remove",
    copy: "Copy",
    copyUrl: "Copy URL",
    copied: "Copied",
    health: {
      checkNow: "Check now",
      checking: "Check started…",
      settings: "Health check settings",
      dialogTitle: "Service health check",
      enabled: "Check this service periodically",
      enabledHint: "Checked every 60 seconds from the host. DNS answers keep working even if the check fails.",
      external: "Allow connecting to external CNAME targets",
      externalHint: "If enabled, the host's external IP and check time may be exposed to the forwarding service.",
      kind: "Check method",
      tcp: "TCP connect only",
      httpHead: "HTTP HEAD (body not fetched)",
      path: "Check path",
      expected: "Expected status",
      notChecked: "Not checked",
      checked: (time: string) => "Last checked " + time,
      openWarning: "The response can't currently be verified. The URL can still be opened.",
      status: {
        healthy: "Up",
        unhealthy: "Not responding",
        unknown: "Not checked",
        disabled: "Check off",
      },
      reason: {
        not_checked: "Not checked yet",
        offline: "The target member is offline",
        timeout: "No response within 3 seconds",
        connection_failed: "Couldn't connect",
        name_resolution_failed: "Couldn't resolve the forward target",
        unexpected_status: "Not the expected HTTP status",
        disabled: "Periodic checking is disabled",
      },
    },
  },

  invite: {
    resultTitle: (name: string) => `Invite for ${name}`,
    warn: (
      <>
        This token is only shown <strong>on this screen</strong>. Give it to that
        person only, and delete it after handing it over.
      </>
    ),
    allocatedIp: "Assigned IP",
    endpoints: "Candidate endpoints",
    psk: "Pre-shared key",
    expires: "Expiry",
    never: "Never",
    yes: "Yes",
    no: "No",
    copied: "Copied",
    copy: "Copy token",
    // Invite deep link (M3-5)
    copyLink: "Copy join link",
    copyLinkHint: "A join link (peercove://…) opens the join form with a single click for anyone who already has PeerCove installed. Give the token instead to people who don't have it installed.",
    resultNote: "Members on the same LAN connect via the LAN candidate; members on a different network use the external candidate. To revoke, remove them from the member list.",
    formTitle: "Invite member",
    nameLabel: "Name (auto-assigned if omitted)",
    namePlaceholder: "alice",
    externalLabel: "External endpoint (for inviting from outside the LAN, e.g. a phone on mobile data)",
    externalHint:
      "The LAN endpoint is filled in automatically. If UPnP is enabled, the external IP is also added automatically. " +
      "If UPnP isn't available, enter \"public IP:listen port\" and forward that UDP port to this PC on your router.",
    externalPlaceholder: "203.0.113.5:51822",
    pskLabel: "Also issue a pre-shared key (PSK)",
    expiryLabel: "Invite expiry",
    expiryHour: "1 hour",
    expiryDay: "1 day",
    expiryWeek: "7 days (recommended)",
    expiryMonth: "30 days",
    expiryHint: "After it expires, the host will reject connections from devices that haven't joined yet.",
    issuing: "Issuing…",
    issue: "Issue invite",
    // Invite issuance by members (ADR-0048)
    memberFormNote: "Asks the host to issue an invite. The issuance is recorded in the host's log and announced in the group chat. The maximum expiry is 7 days.",
    memberResultNote: "The host automatically decides the candidate endpoints. Ask the host to revoke it.",
  },

  logs: {
    title: "Daemon logs",
    level: "Level shown",
    levelOption: (level: string) => `${level} and above`,
    follow: "Follow latest",
    clear: "Clear view",
    dropped: (n: number) => `${n} line${n === 1 ? "" : "s"} overflowed the buffer and were lost.`,
    empty: "No logs yet.",
    emptyForLevel: "No lines match this level.",
    footer: (
      <>
        If the daemon is filtering with <code>--log-level</code>, lines more
        detailed than that won't show up here either. Restart the daemon with{" "}
        <code>--log-level debug</code> to see more detail.
      </>
    ),
  },

  backup: {
    title: "Backup and restore",
    description: "Saves config, keys, DNS, ACL, and groups, encrypted with a passphrase. Does not include chat, received files, quality history, logs, or diagnostic results.",
    createTitle: "Create a backup",
    restoreTitle: "Restore from a backup",
    network: "Network",
    passphrase: "Passphrase",
    confirm: "Passphrase (confirm)",
    passphraseHint: "Enter at least 12 characters. The passphrase itself is not saved.",
    passphraseLength: "The passphrase must be at least 12 characters.",
    passphraseMismatch: "The confirmation passphrase doesn't match.",
    create: "Encrypt and save",
    created: (path: string) => `Backup saved: ${path}`,
    choose: "Choose a backup",
    preview: "Preview contents",
    role: "Role",
    host: "Host",
    member: "Member",
    sourceOs: "Source OS",
    categories: "Included items",
    // Bundling shared notes (M5 F-3, ADR-0049). Shown only when a host config is selected
    includeMemos: "Include shared notes",
    // Display names for categories (Rust side: crates/peercove-ops/src/backup.rs)
    category: {
      network_config: "Network config",
      keys: "Keys",
      groups: "Groups",
      memos: "Shared notes",
      dns: "DNS records",
      acl: "Access control (ACL)",
      invite_metadata: "Invite records",
    },
    restoreName: "Name after restore",
    replace: "Replace an existing network with the same name",
    replaceHint: "Even when replacing, the switch happens only after the restored content is validated. Disconnect the running network first.",
    restore: "Restore",
    restored: "Network restored.",
    rotateRecommendation: "For member configs restored on a different device, we recommend rotating the device key after connecting.",
  },

  settings: {
    title: "Settings",
    mtuInteger: "Enter an integer for the MTU",
    portInteger: "Enter an integer for the listen port",
    savedRestart: "Saved. Reconnects automatically within a few seconds to apply.",
    savedLive: "Saved. Applies to the tunnel within a few seconds.",
    interface: "Interface",
    // Display name / DNS name changes were consolidated into the member list (ADR-0027, M3-19)
    nameMovedHint: "The display name and DNS name can be changed from the ✎ on your own row in the \"Members\" list.",
    portLabel: "Listen port (UDP)",
    portHint: (isMember: boolean, defaultPort: number) => ` — leave empty for ${isMember ? "the OS to choose" : `the default ${defaultPort}`}`,
    mtuLabel: "MTU",
    mtuHint: (defaultMtu: number) => ` — default ${defaultMtu}. Lowering it can help stability on some connections`,
    hostEndpointLabel: "Host endpoint",
    hostEndpointHint: " — the host's IP:port. Used to update it after the host moves",
    hostEndpointPlaceholder: "203.0.113.5:51820",
    // Direct member-to-member communication toggle (ADR-0013 condition 2)
    directLabel: "Communicate directly with members (recommended)",
    directHint: "— connects directly to the other member when possible (faster, no load on the host's connection). Your public IP is shared with members on the network. When off, always routes via the host. Takes about 10 seconds to apply",
    inviteApprovalLabel: "Require approval for new devices to join",
    inviteApprovalHint: "— when checked, a new device connecting via an invite link can do nothing but the approval exchange with the host until the host approves it (it can't reach other members or other host services either). It waits until the host \"approves\" it",
    // Invite issuance by members (ADR-0048)
    memberInvitesLabel: "Allow members to issue invites",
    memberInvitesHint: "— only devices you allow (designated in member details) can issue invites. Turning this OFF disables it for every device",
    // Network management (delegation, ADR-0059 J-2 expansion). Shown on the settings page of admin members
    adminNetworkTitle: "Network management (delegated)",
    adminNetworkHint: "Since admin is delegated to you, you can change the host's network settings from here. It takes a few seconds to apply.",
    networkDisplayNameLabel: "Network display name",
    networkDisplayNameHint: "— leave empty to return to the default display",
    // Received file size limit (ADR-0015, M3-9)
    maxFileLabel: "Received file size limit (MB)",
    maxFileHint: (defaultMb: number) => ` — default ${defaultMb}. 0 for no limit. Files exceeding this are rejected on receipt (applies in about 10 seconds)`,
    maxFileInteger: "Enter an integer of 0 or more for the received file size limit",
    // Auto-reconnect on startup (ADR-0057, M6 H-8e)
    autoConnectLabel: "Connect automatically on startup",
    autoConnectHint: "— automatically connects to this network when this machine starts up (when the PeerCove service starts). Doesn't affect the current connection state.",
    restartHint: (isMember: boolean) =>
      `The listen port, MTU${
        isMember ? ", and host endpoint" : ""
      } are applied by automatically rebuilding the tunnel after saving (within a few seconds).`,
    // Device key rotation (ADR-0020, M3-11)
    rotateKeyLabel: "Device key",
    rotateKeyHint: "Regenerates this device's key and re-registers it with the host. This happens automatically once right after joining, so it's usually not needed (e.g. use it if you're worried an invite token leaked).",
    rotateKeyButton: "Rotate key",
    rotateKeyConfirm: "Rotate the device key? Communication will be interrupted for a few seconds during the switch.",
    rotateKeyRequested: "Requested a key rotation. A brief reconnect will happen once it's done (see the logs for the result).",
    // Storage management (M6 H-10a. Ported from the end of Android's settings tab to desktop)
    storageTitle: "Storage management",
    storageInbox: "Inbox",
    storageChat: "Chat history",
    storageQuality: "Quality history",
    storageMemo: "Shared notes",
    storageMemoHint: "— shared notes storage is managed from the limit settings on the \"Shared\" screen",
    storageClear: "Clear",
    storageClearedInbox: (count: number) => `Emptied the inbox (${count} item${count === 1 ? "" : "s"})`,
    storageCleared: "Cleared.",
    storageChatDisabledHint: "— can only be deleted while connected",
    storageConfirmInboxTitle: "Empty the inbox",
    storageConfirmInbox: "Deletes all files in the inbox. Files currently being received are skipped. Continue?",
    storageConfirmChatTitle: "Delete chat history",
    storageConfirmChat: "Deletes all chat history stored on this device (the other side's history is unaffected). Continue?",
  },

  // OS notifications (notify.ts)
  notify: {
    approvalTitle: "PeerCove: New join request",
    joinedTitle: "A member joined",
    leftTitle: "A member disconnected",
    body: (name: string, ip: string, network: string) => `${name} (${ip}) — ${network}`,
    // File received (M3-9b)
    fileTitle: "File received",
    fileBody: (file: string, from: string, network: string) => `${file} — from ${from} (${network})`,
    // New chat message (M3-13b). Show the body in the notification, same as LINE
    chatTitle: (from: string, network: string) => `${from} (${network})`,
    chatBody: (text: string, isAll: boolean) => (isAll ? `[All] ${text}` : text),
    // Chat mentions (ADR-0055 decision 1). Replace the title for messages that mention you (@name / @All) to make it clearer
    chatMentionTitle: (from: string, network: string) => `${from} mentioned you (${network})`,
    // Shared note comments/mentions (M5 F-5 Stage 3, ADR-0052 decisions 4/5).
    // Notifications are local display, not logs, so it's fine to include the note title and comment body
    commentTitle: (from: string, memoTitle: string) => `${from} commented on "${memoTitle}"`,
    mentionTitle: (from: string, memoTitle: string) => `${from} mentioned you (in "${memoTitle}")`,
    // Local announcement line in chat (ADR-0055 decision 1d). Including an @memo:id
    // token turns it into a card in chat, clickable to jump to the note
    chatNoteMention: (from: string, memoTitle: string, token: string) => `📝 ${from} mentioned you in the note "${memoTitle}" ${token}`,
    chatNoteComment: (from: string, memoTitle: string, token: string) => `📝 ${from} commented on your note "${memoTitle}" ${token}`,
    // Note reminder (local to device, M5 F-5 Stage 5, ADR-0052 decision 6)
    reminderTitle: (memoTitle: string) => `⏰ Note reminder: ${memoTitle}`,
    // Event reminder (local to device, M6 H-3b, ADR-0055 decision 3)
    scheduleReminderTitle: (title: string, hm: string) => `⏰ Event: ${title} (${hm})`,
  },

  // Personal notes (M5 F-1, ADR-0049)
  memo: {
    title: "Notes",
    newMemo: "New note",
    searchPlaceholder: "Search (title and body)",
    scopeActive: "Notes",
    scopeArchived: "Archive",
    scopeTrash: "Trash",
    sortLabel: "Sort",
    sortUpdated: "Last updated",
    sortCreated: "Date created",
    sortTitle: "Title",
    folders: "Folders",
    allMemos: "All notes",
    noFolder: "No folder",
    newFolder: "New folder",
    folderNamePlaceholder: "Folder name",
    renameFolder: "Rename folder",
    renamePrompt: "New folder name",
    deleteFolder: "Delete folder",
    folderDeleteConfirm: (name: string) => `Delete folder "${name}"? (Notes inside move to "No folder")`,
    empty: "No notes. Create one with \"+ New note\"",
    emptyTrash: "Trash is empty",
    untitled: "Untitled",
    selectPrompt: "Choose a note from the list on the left, or create a new one",
    loadFailed: "Couldn't load notes. Check whether the daemon is running and not an old version",
    inTrash: "Note in trash (read-only)",
    pin: "Pin",
    unpin: "Unpin",
    archive: "Move to archive",
    unarchive: "Restore from archive",
    duplicate: "Duplicate",
    toTrash: "Move to trash",
    restore: "Restore",
    deleteForever: "Delete permanently",
    deleteForeverConfirm: "Permanently delete this note? (This can't be undone)",
    emptyTrashAction: "Empty trash",
    emptyTrashConfirm: "Permanently delete all notes in the trash? (This can't be undone)",
    import: "Import text",
    importNote: "Imports UTF-8 .txt files as personal notes (the file name becomes the title)",
    imported: (n: number) => `Imported ${n} text file${n === 1 ? "" : "s"}`,
    exportNote: "Saves the body as UTF-8 text (tags and history are not included)",
    exported: (path: string) => `Saved: ${path}`,
    modeEdit: "Edit",
    modePreview: "Preview",
    modeSplit: "Split",
    titlePlaceholder: "Title",
    bodyPlaceholder: "Body (Markdown supported)",
    saved: "Saved",
    saving: "Saving…",
    saveFailed: "Save failed",
    stats: (chars: number, lines: number) => `${chars} chars, ${lines} lines`,
    updatedAt: (date: string) => `Updated: ${date}`,
    tagsPlaceholder: "Tags (comma-separated)",
    folderLabel: "Folder",
    fmtHeading: "Heading",
    fmtBold: "Bold",
    fmtItalic: "Italic",
    fmtStrike: "Strikethrough",
    fmtList: "Bullet list",
    fmtCheck: "Checklist",
    fmtQuote: "Quote",
    fmtCode: "Inline code",
    fmtCodeBlock: "Code block",
    fmtTable: "Table",
    fmtLink: "Link",
    fmtHr: "Horizontal rule",
    // Copy a personal note into a shared note (M5 F-3, ADR-0049)
    copyToShared: "Copy to shared notes",
    copyToSharedChoose: "Choose destination network",
    copiedToShared: "Copied to shared notes",
    // Note links / backlinks (M5 F-5 Stage 2, ADR-0052 decision 2)
    wikilinkMissing: "The linked note couldn't be found",
    backlinksTitle: (n: number) => `Backlinks (${n})`,

    // Reminder (local to device, M5 F-5 Stage 5, ADR-0052 decision 6).
    // Shared notes (SharedMemoView) use this same key too (same convention as t.memo.toTrash)
    reminder: "Reminder",
    reminderTitle: "Set a reminder",
    reminderLabel: "Date and time",
    reminderSave: "Set",
    reminderClear: "Clear",
    reminderSaved: "Reminder set",
    reminderCleared: "Reminder cleared",
    reminderAt: (date: string) => `Reminder: ${date}`,
  },

  // Shared hub (M5 F-5 Stage 1, ADR-0052 decision 3). Currently the only sub-tab
  // is "notes"; add keys here later for schedule/sheets
  sharedHub: {
    tabMemos: "Notes",
    tabSchedule: "Schedule",
    tabSheets: "Sheets",
  },

  // Shared notes (M5 F-2, ADR-0049)
  sharedMemo: {
    title: "Shared notes",
    loadFailed: "Couldn't load shared notes. Check whether the daemon is running and not an old version",
    offline: "Read-only while offline (not connected to the host)",
    unsupported: "Not synced with the host (the host may be on an old version)",
    scopeAll: "Shared notes",
    folders: "Shared folders",
    empty: "No shared notes",
    selectPrompt: "Choose a shared note from the list on the left, or create a new one",
    viewing: "Viewing (live updates)",
    editingBy: (name: string) => `${name} is editing`,
    startEdit: "Edit",
    stopEdit: "Stop editing",
    forceUnlock: "Force unlock",
    forceUnlockConfirm: "Force unlock the editing lock? (The editor may have unsaved changes)",
    perms: "Permission settings",
    permsTitle: "Shared notes permissions",
    permsNote: "Permissions are tied to member identifiers or groups. Priority is \"individual member > group > everyone\" (use \"Hide\" to exclude a specific member/group).",
    everyoneLabel: "Everyone (all members of the network)",
    groupsLabel: "Groups",
    groupLevelInherit: "Not set",
    unknownGroupBadge: "(unknown group)",
    levelViewer: "View only",
    levelEditor: "View + edit",
    levelNone: "Hide",
    levelInherit: "Follow everyone",
    viewerBadge: "View only",
    copyToPersonal: "Copy as a personal note",
    copiedToPersonal: "Copied to personal notes (open it from the Notes tab)",
    ownerLabel: (name: string) => `Owner: ${name}`,
    hostName: "Host",
    updatedBy: (name: string) => `Updated by: ${name}`,
    plaintextNote: "Shared notes are stored on the host device in plain text. Don't use them to store passwords, private keys, invite tokens, or similar.",
    // Note links / backlinks (M5 F-5 Stage 2, ADR-0052 decision 2)
    wikilinkMissing: "The linked note couldn't be found",
    backlinksTitle: (n: number) => `Backlinks (${n})`,

    // Pinning (local to device, M6 H-9b. Changed from shared-by-everyone to
    // device-local per requester feedback: what matters differs by person)
    pinLocal: "Pin on this device (an order visible only to you)",
    unpinLocal: "Unpin on this device",

    // Copy a link to chat `@memo:id` (M5 F-5 Stage 4, ADR-0052 decision 1)
    copyLink: "Copy link",
    copyLinkDone: "Copied a link you can paste into chat",

    // Version history (M5 F-3, ADR-0049)
    history: "History",
    historyEmpty: "No change history",
    historySelectPrompt: "Choose a version from the list on the left",
    historyKind: {
      auto: "Auto-saved",
      close: "Finished editing",
      manual: "Manually saved",
      restore: "Before restore",
    },
    historyCompare: "Compare with current",
    historyShowBody: "Show body",
    historyRestore: "Restore this version",
    historyRestoreConfirm: "This replaces the current body with this version's content. The current content is kept in history.",
    historyRestored: "Restored to this version's content",
    saveVersion: "Save version",
    saveVersionDone: "Version saved",

    // Storage / history limits (admin (host/admin) only, M5 F-3. Expanded by ADR-0059 J-2)
    limits: "Limit settings",
    limitsTitle: "Shared notes limits",
    limitsNote: "Sets the storage size, item count, and history retention limits for shared notes. Values outside the allowed range will error when saving.",
    limitsBodyLabel: "Body size limit per note (KB)",
    limitsTotalLabel: "Total storage limit (MB, body + history)",
    limitsCountLabel: "Note count limit (including trash)",
    limitsVersionsLabel: "Change history entries kept per note",
    limitsHistoryDaysLabel: "Change history retention (days)",
    limitsTrashDaysLabel: "Trash retention (days)",
    limitsSaved: "Limits saved",

    // Comments / mentions (M5 F-5 Stage 3, ADR-0052 decisions 4/5)
    commentsTitle: (n: number) => `Comments (${n})`,
    commentsEmpty: "No comments yet",
    commentPlaceholder: "Write a comment (@name to mention)",
    commentSend: "Send",
    commentDelete: "Delete",
    commentDeleteConfirm: "Delete this comment? (This can't be undone)",
    commentLoadFailed: "Couldn't load comments",
    commentBadge: (n: number) => `💬${n}`,
    // @All mention suggestion (ADR-0055 decisions 1b/1c). The notify-everyone
    // behavior is implemented in H-2; here it's just offered as a suggestion.
    mentionAllLabel: "All (everyone)",
  },

  // Shared schedule (M6 G-1, ADR-0053)
  schedule: {
    title: "Schedule",
    loadFailed: "Couldn't load the schedule. Check whether the daemon is running and not an old version",
    offline: "Read-only while offline (not connected to the host)",
    unsupported: "Not synced with the host (the host may be on an old version)",
    today: "Today",
    weekdays: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
    addEvent: "Add event",
    editEvent: "Edit",
    deleteEvent: "Delete",
    deleteConfirm: "Delete this event? (This can't be undone)",
    more: (n: number) => `+${n}`,
    allDayLabel: "All day",
    titleLabel: "Title",
    titlePlaceholder: "Enter a title",
    noteLabel: "Notes",
    notePlaceholder: "Notes (optional)",
    startLabel: "Start",
    endLabel: "End (optional)",
    createTitle: "Add event",
    editTitle: "Edit event",
    detailTitle: "Event details",
    ownerLabel: (name: string) => `Created by: ${name}`,
    updatedByLabel: (name: string) => `Updated by: ${name}`,
    noEventsForDay: "No events on this day",
    selectDayPrompt: "Pick a date on the calendar to see its events",
    copyLink: "Copy link",
    copyLinkDone: "Copied a link you can paste into chat",
    conflictNotice: "Another device's change was already saved. Please reload",
    excerptAllDay: (md: string) => `${md} all day`,
    excerptTimed: (md: string, hm: string) => `${md} ${hm}`,
    viewerBadge: "View only",

    // Participants (ADR-0055 decision 5)
    participantsLabel: "Participants",
    participantsBadgeCount: (n: number) => `👥${n}`,
    selfBadge: "(you)",
    filterAll: "All",
    filterMine: "My events",

    // Calendar holidays (ADR-0055 decision 4)
    holidayBadgeTitle: (name: string) => name,

    // Event reminders (local to device, multiple allowed, ADR-0055 decision 3)
    remindersTitle: "⏰ Reminders",
    reminderEmpty: "No reminders set",
    reminderPreset5: "5 minutes before",
    reminderPreset15: "15 minutes before",
    reminderPreset30: "30 minutes before",
    reminderPreset60: "1 hour before",
    reminderPreset1440: "1 day before",
    reminderPresetGeneric: (n: number) => `${n} minutes before`,
    reminderOffsetLabel: (preset: string, hm: string) => `${preset} (${hm})`,
    reminderAdd: "Add",
    reminderLimitReached: "You can set up to 10 reminders",
    reminderHelp: "Existing reminder times don't follow along if you change the event's date/time",
  },

  // Shared sheets (Excel-like table, M6 G-2, ADR-0054)
  sheet: {
    title: "Sheets",
    loadFailed: "Couldn't load the sheet. Check whether the daemon is running and not an old version",
    offline: "Read-only while offline (not connected to the host)",
    unsupported: "Not synced with the host (the host may be on an old version)",
    empty: "No sheets yet",
    addSheet: "Create sheet",
    moreOptions: "More actions",
    newSheetNamePrompt: "Enter a sheet name",
    newSheetDefaultName: "Sheet1",
    renameSheet: "Rename sheet",
    renamePrompt: "Enter a new sheet name",
    deleteSheet: "Delete sheet",
    deleteConfirm: (name: string) => `Delete sheet "${name}"? (This can't be undone)`,
    copyLink: "Copy link",
    copyLinkDone: "Copied a link you can paste into chat",
    conflictNotice: (n: number) => `Another device updated some cells first (${n})`,
    pasteRejected: "The paste range exceeds the sheet's limits",
    // Applying formatting to wide ranges like whole columns/rows (M6 H-8a, ADR-0055 decision 6 follow-up)
    formatLargeSelectionNotice: "The selection is large, so formatting was applied only to cells with a value",
    ownerLabel: (name: string) => `Created by: ${name}`,
    // Formatting toolbar (M6 H-4, ADR-0055 decision 6)
    toolbarBold: "Bold",
    toolbarItalic: "Italic",
    toolbarUnderline: "Underline",
    toolbarStrike: "Strikethrough",
    toolbarFontSize: "Font size",
    toolbarTextColor: "Text color",
    toolbarBgColor: "Background color",
    toolbarColorNone: "None",
    toolbarAlignLeft: "Align left",
    toolbarAlignCenter: "Align center",
    toolbarAlignRight: "Align right",
    toolbarBorderOuter: "Outer border",
    toolbarBorderGrid: "Grid",
    toolbarBorderNone: "No border",
    resetToDefault: "Reset to default size (double-click)",
    // Click the top-left corner to select all cells (M6 H-8a, ADR-0055 decision 6 follow-up)
    selectAllTitle: "Select all",
    // Cell merging (M6 H-5, ADR-0055 decision 6)
    toolbarMerge: "Merge",
    toolbarUnmerge: "Unmerge",
    mergeConfirmDataLoss: "Merging will lose the contents of all but the top-left cell. Continue?",
    // Sheet settings: gridlines, freeze panes (M6 H-5, ADR-0055 decision 6)
    toolbarGridlines: "Gridlines",
    toolbarFreeze: "Freeze above/left of selection",
    toolbarUnfreeze: "Unfreeze",
    // Custom right-click menu (M6 H-5, ADR-0055 decision 6)
    ctxCopy: "Copy",
    ctxPaste: "Paste",
    ctxClear: "Clear",
    ctxMerge: "Merge",
    ctxUnmerge: "Unmerge",
    ctxResetColWidth: "Reset column width to default",
    ctxResetRowHeight: "Reset row height to default",
    clipboardReadUnavailable: "Can't read the clipboard. Please use Ctrl+V",
    // In-sheet search (Ctrl+F, M6 H-5, ADR-0055 decision 6)
    searchPlaceholder: "Search in sheet",
    searchCount: (current: number, total: number) => `${current} / ${total}`,
    searchNoMatch: "No matches",
    searchPrev: "Previous",
    searchNext: "Next",
  },
};

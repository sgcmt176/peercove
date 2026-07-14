import { useEffect, useMemo, useState } from "react";
import { AclPolicySettings, AclProtocol, AclRule, AclTarget, DnsRecord, Member, api, errorMessage } from "../ipc";
import { t } from "../i18n";

type TargetKind = "any" | "member" | "group" | "subnet" | "service";

export function AclView({ configPath, members }: { configPath: string; members: Member[]; }) {
  const [policy, setPolicy] = useState<AclPolicySettings | null>(null);
  const [services, setServices] = useState<DnsRecord[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [action, setAction] = useState<"allow" | "deny">("deny");
  const [sourceKind, setSourceKind] = useState<TargetKind>("any");
  const [sourceValue, setSourceValue] = useState("");
  const [destinationKind, setDestinationKind] = useState<TargetKind>("member");
  const [destinationValue, setDestinationValue] = useState("");
  const [protocol, setProtocol] = useState<AclProtocol>("any");
  const [ports, setPorts] = useState("");
  const [groupName, setGroupName] = useState("");
  const [groupMembers, setGroupMembers] = useState<Set<string>>(new Set());

  const peers = members.filter((member) => !member.isHost);
  const usableServices = services.filter((record) => record.id && !record.cname && record.port);
  useEffect(() => {
    let alive = true;
    setPolicy(null);
    setServices([]);
    setError(null);
    Promise.all([api.readAclPolicy(configPath), api.listDnsRecords(configPath)])
      .then(([loaded, dns]) => {
        if (!alive) return;
        // 初期のACL v2実装は既定値のports/enabledをJSONから省略した。
        // 旧UIバックエンドからの応答でもページ全体を落とさないよう補完する。
        setPolicy({
          ...loaded,
          groups: loaded.groups ?? [],
          rules: (loaded.rules ?? []).map((rule) => ({
            ...rule,
            ports: rule.ports ?? [],
            enabled: rule.enabled ?? true,
          })),
        });
        setServices(dns);
        setDestinationValue(peers[0]?.publicKey ?? "");
      })
      .catch((cause) => {
        if (alive) setError(errorMessage(cause));
      });
    return () => { alive = false; };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [configPath]);

  const save = async (next: AclPolicySettings) => {
    setBusy(true); setError(null);
    try { await api.writeAclPolicy(configPath, next); setPolicy(next); }
    catch (cause) { setError(errorMessage(cause)); }
    finally { setBusy(false); }
  };

  const target = (kind: TargetKind, value: string): AclTarget => {
    if (kind === "any") return "any";
    return { [kind]: value } as AclTarget;
  };
  const labelTarget = (value: AclTarget) => {
    if (value === "any") return t.acl.any;
    if ("member" in value) return peers.find((m) => m.publicKey === value.member)?.name ?? value.member.slice(0, 8);
    if ("group" in value) return `${t.acl.group}: ${value.group}`;
    if ("subnet" in value) return value.subnet;
    return usableServices.find((s) => s.id === value.service)?.fqdn ?? value.service;
  };
  const portsList = ports.split(",").map((value) => value.trim()).filter(Boolean);
  const preview = `${action === "allow" ? t.acl.allow : t.acl.deny}：${labelTarget(target(sourceKind, sourceValue))} → ${labelTarget(target(destinationKind, destinationValue))} / ${protocol.toUpperCase()}${portsList.length ? ` ${portsList.join(", ")}` : ""}`;

  const addRule = () => {
    if (!policy) return;
    if (sourceKind !== "any" && !sourceValue) { setError(t.acl.targetRequired); return; }
    if (destinationKind !== "any" && !destinationValue) { setError(t.acl.targetRequired); return; }
    const rule: AclRule = { id: `rule-${Date.now().toString(36)}`, action, source: target(sourceKind, sourceValue), destination: target(destinationKind, destinationValue), protocol, ports: portsList, enabled: true };
    void save({ ...policy, rules: [...policy.rules, rule] });
  };
  const updateRule = (index: number, change: Partial<AclRule>) => {
    if (!policy) return;
    const rules = policy.rules.map((rule, i) => i === index ? { ...rule, ...change } : rule);
    void save({ ...policy, rules });
  };
  const move = (index: number, offset: number) => {
    if (!policy || index + offset < 0 || index + offset >= policy.rules.length) return;
    const rules = [...policy.rules];
    [rules[index], rules[index + offset]] = [rules[index + offset], rules[index]];
    void save({ ...policy, rules });
  };
  const groupId = useMemo(() => groupName.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 63), [groupName]);
  const addGroup = () => {
    if (!policy || !groupId || groupMembers.size === 0) return;
    void save({ ...policy, groups: [...policy.groups, { id: groupId, members: [...groupMembers] }] });
    setGroupName(""); setGroupMembers(new Set());
  };

  return <div className="acl-page">
    <section className="card acl-page__head">
      <h2 className="card-title">{t.acl.title}</h2>
      <p className="muted small">{t.acl.introV2}</p>
    </section>
    <section className="card acl-v2">
      {!policy ? !error && <p className="muted">{t.common.loading}</p> : <>
        <div className="acl-v2__default"><span>{t.acl.defaultAction}</span><select value={policy.default} disabled={busy} onChange={(event) => void save({ ...policy, default: event.target.value as "allow" | "deny" })}><option value="allow">{t.acl.allow}</option><option value="deny">{t.acl.deny}</option></select></div>
        <section><h3>{t.acl.rules}</h3>
          {policy.rules.length === 0 ? <p className="muted small">{t.acl.noRules}</p> : <div className="acl-v2__table-wrap"><table className="acl-v2__table"><thead><tr><th>{t.acl.order}</th><th>{t.acl.action}</th><th>{t.acl.source}</th><th>{t.acl.destination}</th><th>{t.acl.protocol}</th><th>{t.acl.ports}</th><th>{t.acl.state}</th><th>{t.acl.actions}</th></tr></thead><tbody>{policy.rules.map((rule, index) => <tr key={rule.id}><td>{index + 1}</td><td><span className={`tag ${rule.action === "deny" ? "tag--blocked" : ""}`}>{rule.action === "allow" ? t.acl.allow : t.acl.deny}</span></td><td>{labelTarget(rule.source)}</td><td>{labelTarget(rule.destination)}</td><td>{rule.protocol.toUpperCase()}</td><td>{rule.ports.join(", ") || t.acl.allPorts}</td><td><label className="chat__pick-row"><input type="checkbox" checked={rule.enabled} disabled={busy} onChange={(event) => updateRule(index, { enabled: event.target.checked })}/><span>{rule.enabled ? t.acl.enabled : t.acl.disabled}</span></label></td><td><div className="row"><button type="button" className="button--icon" aria-label={t.acl.moveUp} disabled={busy || index === 0} onClick={() => move(index, -1)}>↑</button><button type="button" className="button--icon" aria-label={t.acl.moveDown} disabled={busy || index === policy.rules.length - 1} onClick={() => move(index, 1)}>↓</button><button type="button" className="button--icon" aria-label={t.common.delete} disabled={busy} onClick={() => void save({ ...policy, rules: policy.rules.filter((_, i) => i !== index) })}>×</button></div></td></tr>)}</tbody></table></div>}
        </section>
        <section className="acl-v2__builder"><h3>{t.acl.addRule}</h3><div className="acl-v2__form">
          <label>{t.acl.action}<select value={action} onChange={(e) => setAction(e.target.value as "allow" | "deny")}><option value="deny">{t.acl.deny}</option><option value="allow">{t.acl.allow}</option></select></label>
          <TargetInput label={t.acl.source} kind={sourceKind} value={sourceValue} setKind={setSourceKind} setValue={setSourceValue} members={peers} groups={policy.groups.map((g) => g.id)} services={usableServices} allowService={false}/>
          <TargetInput label={t.acl.destination} kind={destinationKind} value={destinationValue} setKind={setDestinationKind} setValue={setDestinationValue} members={peers} groups={policy.groups.map((g) => g.id)} services={usableServices} allowService/>
          <label>{t.acl.protocol}<select value={protocol} onChange={(e) => { setProtocol(e.target.value as AclProtocol); if (!['tcp','udp'].includes(e.target.value)) setPorts(""); }}><option value="any">ANY</option><option value="tcp">TCP</option><option value="udp">UDP</option><option value="icmp">ICMP</option></select></label>
          {(protocol === "tcp" || protocol === "udp") && <label>{t.acl.ports}<input value={ports} placeholder="443, 8000-8100" onChange={(e) => setPorts(e.target.value)}/></label>}
        </div><p className="acl-v2__preview">{preview}</p><p className="notice small">{t.acl.relayWarning}</p><button type="button" disabled={busy} onClick={addRule}>{t.acl.addRule}</button></section>
        <section className="acl-v2__groups"><h3>{t.acl.groups}</h3>{policy.groups.map((group) => <div className="row" key={group.id}><strong>{group.id}</strong><span className="muted small">{group.members.length}{t.acl.people}</span><button type="button" className="button--ghost" onClick={() => void save({ ...policy, groups: policy.groups.filter((g) => g.id !== group.id), rules: policy.rules.filter((r) => !(typeof r.source !== 'string' && 'group' in r.source && r.source.group === group.id) && !(typeof r.destination !== 'string' && 'group' in r.destination && r.destination.group === group.id)) })}>{t.common.delete}</button></div>)}<label>{t.acl.groupName}<input value={groupName} onChange={(e) => setGroupName(e.target.value)}/></label><div className="acl-v2__member-checks">{peers.map((member) => <label className="chat__pick-row" key={member.publicKey}><input type="checkbox" checked={groupMembers.has(member.publicKey)} onChange={(e) => { const next = new Set(groupMembers); e.target.checked ? next.add(member.publicKey) : next.delete(member.publicKey); setGroupMembers(next); }}/><span>{member.name ?? member.ip}</span></label>)}</div><button type="button" className="button--ghost" disabled={!groupId || groupMembers.size === 0 || busy} onClick={addGroup}>{t.acl.addGroup}</button></section>
      </>}
      {error && <p role="alert" className="error-text small">{error}</p>}
    </section>
  </div>;
}

function TargetInput({ label, kind, value, setKind, setValue, members, groups, services, allowService }: { label: string; kind: TargetKind; value: string; setKind: (kind: TargetKind) => void; setValue: (value: string) => void; members: Member[]; groups: string[]; services: DnsRecord[]; allowService: boolean; }) {
  const selectKind = (next: TargetKind) => { setKind(next); setValue(next === "member" ? members[0]?.publicKey ?? "" : next === "group" ? groups[0] ?? "" : next === "service" ? services[0]?.id ?? "" : ""); };
  return <fieldset className="acl-v2__target"><legend>{label}</legend><select value={kind} onChange={(e) => selectKind(e.target.value as TargetKind)}><option value="any">{t.acl.any}</option><option value="member">{t.acl.member}</option><option value="group">{t.acl.group}</option><option value="subnet">{t.acl.subnet}</option>{allowService && <option value="service">{t.acl.service}</option>}</select>{kind === "member" && <select value={value} onChange={(e) => setValue(e.target.value)}>{members.map((m) => <option key={m.publicKey} value={m.publicKey}>{m.name ?? m.ip}</option>)}</select>}{kind === "group" && <select value={value} onChange={(e) => setValue(e.target.value)}>{groups.map((g) => <option key={g}>{g}</option>)}</select>}{kind === "subnet" && <input value={value} placeholder="10.99.0.0/24" onChange={(e) => setValue(e.target.value)}/>} {kind === "service" && <select value={value} onChange={(e) => setValue(e.target.value)}>{services.map((s) => <option key={s.id!} value={s.id!}>{s.fqdn}:{s.port}</option>)}</select>}</fieldset>;
}

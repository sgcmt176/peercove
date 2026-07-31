import { useState } from "react";
import { Member, PermGroup, api, errorMessage } from "../ipc";
import { t } from "../i18n";
import { ConfirmModal, Modal } from "./Modal";

/**
 * 権限グループの独立画面(ADR-0060 決定5)。通信制御・共有メモなどで
 * 共通に使う「誰を含むか」の単位を、ここ 1 箇所で作成・改名・メンバー編集・
 * 削除する。データ源は `tunnel.permGroups`(host は ACL ポリシー由来、member
 * は台帳配布分。App.tsx の status ポーリングで更新される)。
 *
 * 操作はホスト/admin のみ(DnsDialog/AclDialog と同じ 3 分岐)。ホストは
 * `peercove_ops::acl::perm_group_*` を IPC で直接呼ぶ(members は公開鍵)。
 * admin は `adminOp({ op: "perm_group_*" })` でホストへ中継する(members は
 * 仮想 IP — ホスト側で公開鍵へ解決してもらう。ADR-0060 決定1)。
 */
export function PermGroupView({
  configPath,
  members,
  permGroups,
  isHost,
  isAdmin,
  onChanged,
}: {
  configPath: string;
  members: Member[];
  permGroups: PermGroup[];
  isHost: boolean;
  /** 管理を委任されている(ADR-0059)。ホストでなくても編集できる。 */
  isAdmin: boolean;
  /** 変更を即時反映させたい(host 経路)ときに呼ぶ。親が status を取り直す。 */
  onChanged: () => void;
}) {
  const canManage = isHost || isAdmin;
  const [creating, setCreating] = useState(false);
  const [renaming, setRenaming] = useState<PermGroup | null>(null);
  const [editingMembers, setEditingMembers] = useState<PermGroup | null>(null);
  const [deleting, setDeleting] = useState<PermGroup | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // 仮想 IP → 表示名(解決できなければ IP をそのまま出す — 実装指示書)
  const nameOf = (ip: string) => members.find((m) => m.ip === ip)?.name ?? ip;

  // host は host.toml へ直接反映されるので即座に再取得を促す。admin は
  // ホストへ中継しただけで、実際の反映は次回の台帳配布を待つ(DnsView の
  // adminApplyNotice と同じ考え方をここでも使う)。
  const applied = () => {
    setError(null);
    if (isHost) onChanged();
    else setNotice(t.dns.adminApplyNotice);
  };

  const create = async (name: string, picked: Member[]) => {
    setBusy(true);
    setError(null);
    try {
      if (isHost) {
        await api.permGroupCreate(configPath, name, picked.map((m) => m.publicKey));
      } else {
        await api.adminOp(configPath, {
          op: "perm_group_create",
          name,
          members: picked.map((m) => m.ip),
        });
      }
      setCreating(false);
      applied();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const rename = async (id: string, name: string) => {
    setBusy(true);
    setError(null);
    try {
      if (isHost) await api.permGroupRename(configPath, id, name);
      else await api.adminOp(configPath, { op: "perm_group_rename", id, name });
      setRenaming(null);
      applied();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const setGroupMembers = async (id: string, picked: Member[]) => {
    setBusy(true);
    setError(null);
    try {
      if (isHost) {
        await api.permGroupSetMembers(configPath, id, picked.map((m) => m.publicKey));
      } else {
        await api.adminOp(configPath, {
          op: "perm_group_set_members",
          id,
          members: picked.map((m) => m.ip),
        });
      }
      setEditingMembers(null);
      applied();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      if (isHost) await api.permGroupDelete(configPath, id);
      else await api.adminOp(configPath, { op: "perm_group_delete", id });
      setDeleting(null);
      applied();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="acl-page">
      <section className="card acl-page__head">
        <h2 className="card-title">{t.permGroups.title}</h2>
        <p className="muted small">{t.permGroups.intro}</p>
        {!canManage && <p className="muted small">{t.permGroups.viewOnlyNote}</p>}
      </section>
      <section className="card">
        {permGroups.length === 0 ? (
          <p className="muted small">{t.permGroups.empty}</p>
        ) : (
          <ul className="dns-list">
            {permGroups.map((group) => (
              <li key={group.id} className="dns-list__row">
                <span className="dns-list__names">
                  <strong className="ellipsis">{group.name || group.id}</strong>
                  <span className="muted small">
                    {t.acl.people((group.members ?? []).length)}
                    {(group.members ?? []).length > 0 &&
                      `: ${(group.members ?? []).map(nameOf).join(", ")}`}
                  </span>
                </span>
                {canManage && (
                  <div className="row">
                    <button
                      type="button"
                      className="button--link small"
                      onClick={() => setRenaming(group)}
                    >
                      {t.permGroups.rename}
                    </button>
                    <button
                      type="button"
                      className="button--link small"
                      onClick={() => setEditingMembers(group)}
                    >
                      {t.permGroups.editMembers}
                    </button>
                    <button
                      type="button"
                      className="button--link button--link-danger small"
                      onClick={() => setDeleting(group)}
                    >
                      {t.common.delete}
                    </button>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
        {canManage && (
          <button type="button" onClick={() => setCreating(true)}>
            {t.permGroups.create}
          </button>
        )}
        {notice && <p className="muted small">{notice}</p>}
        {error && <p role="alert" className="error-text small">{error}</p>}
      </section>

      {creating && (
        <GroupFormDialog
          title={t.permGroups.createTitle}
          members={members}
          initialName=""
          initialMemberKeys={new Set()}
          busy={busy}
          showNameField
          onClose={() => setCreating(false)}
          onSave={(name, picked) => void create(name, picked)}
        />
      )}
      {renaming && (
        <RenameDialog
          group={renaming}
          busy={busy}
          onClose={() => setRenaming(null)}
          onSave={(name) => void rename(renaming.id, name)}
        />
      )}
      {editingMembers && (
        <GroupFormDialog
          title={t.permGroups.editMembersTitle}
          members={members}
          initialName={editingMembers.name || editingMembers.id}
          initialMemberKeys={
            new Set(
              members
                .filter((m) => (editingMembers.members ?? []).includes(m.ip))
                .map((m) => m.publicKey),
            )
          }
          busy={busy}
          showNameField={false}
          onClose={() => setEditingMembers(null)}
          onSave={(_name, picked) => void setGroupMembers(editingMembers.id, picked)}
        />
      )}
      {deleting && (
        <ConfirmModal
          title={t.permGroups.deleteTitle}
          message={t.permGroups.deleteConfirm(deleting.name || deleting.id)}
          confirmLabel={t.common.delete}
          busy={busy}
          onClose={() => setDeleting(null)}
          onConfirm={() => void remove(deleting.id)}
        />
      )}
    </div>
  );
}

/** グループ名を変える(メンバーはそのまま)。 */
function RenameDialog({
  group,
  busy,
  onClose,
  onSave,
}: {
  group: PermGroup;
  busy: boolean;
  onClose: () => void;
  onSave: (name: string) => void;
}) {
  const [name, setName] = useState(group.name || group.id);
  return (
    <Modal title={t.permGroups.renameTitle} onClose={onClose}>
      <div className="modal__body">
        <label className="field">
          <span>{t.permGroups.nameLabel}</span>
          <input
            value={name}
            autoFocus
            maxLength={64}
            disabled={busy}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
        <button
          type="button"
          disabled={busy || name.trim() === ""}
          onClick={() => onSave(name.trim())}
        >
          {busy ? t.common.saving : t.common.save}
        </button>
      </div>
    </Modal>
  );
}

/**
 * グループ作成 / メンバー編集の共通フォーム。ホストもチェックボックスの
 * 候補に含める(ADR-0060 決定2)。名前欄は作成時だけ編集可(メンバー編集
 * では既存名をそのまま表示するだけ)。
 */
function GroupFormDialog({
  title,
  members,
  initialName,
  initialMemberKeys,
  busy,
  showNameField,
  onClose,
  onSave,
}: {
  title: string;
  members: Member[];
  initialName: string;
  initialMemberKeys: Set<string>;
  busy: boolean;
  showNameField: boolean;
  onClose: () => void;
  onSave: (name: string, picked: Member[]) => void;
}) {
  const [name, setName] = useState(initialName);
  const [picked, setPicked] = useState<Set<string>>(initialMemberKeys);

  const toggle = (publicKey: string, checked: boolean) => {
    const next = new Set(picked);
    if (checked) next.add(publicKey);
    else next.delete(publicKey);
    setPicked(next);
  };

  const canSave = (!showNameField || name.trim() !== "") && picked.size > 0;

  return (
    <Modal title={title} onClose={onClose}>
      <div className="modal__body">
        {showNameField ? (
          <label className="field">
            <span>{t.permGroups.nameLabel}</span>
            <input
              value={name}
              autoFocus
              maxLength={64}
              placeholder={t.permGroups.namePlaceholder}
              disabled={busy}
              onChange={(event) => setName(event.target.value)}
            />
          </label>
        ) : (
          <p className="mono">{name}</p>
        )}
        <span className="field__label">{t.permGroups.membersPick}</span>
        <div className="acl-v2__member-checks">
          {members.map((member) => (
            <label className="chat__pick-row" key={member.publicKey}>
              <input
                type="checkbox"
                checked={picked.has(member.publicKey)}
                disabled={busy}
                onChange={(event) => toggle(member.publicKey, event.target.checked)}
              />
              <span>{member.name ?? member.ip}</span>
            </label>
          ))}
        </div>
        <p className="muted small">{t.permGroups.hostIncludeNote}</p>
        {!canSave && <p className="muted small">{t.permGroups.createDisabledHint}</p>}
      </div>
      <div className="modal__actions">
        <button type="button" className="button--ghost" onClick={onClose}>
          {t.common.cancel}
        </button>
        <button
          type="button"
          disabled={busy || !canSave}
          onClick={() =>
            onSave(
              name.trim(),
              members.filter((m) => picked.has(m.publicKey)),
            )
          }
        >
          {busy ? t.common.saving : t.common.save}
        </button>
      </div>
    </Modal>
  );
}

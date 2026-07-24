// 共有ハブ(M5 F-5 Stage 1、ADR-0052 決定 3)。共有系機能をタブで増やし続け
// ず「共有」1 か所にまとめる器。内部はサブタブで切り替える。現在はサブタブ
// 「メモ」のみだが、今後スケジュール表・Excel ライク表を足すときは
// TABS 配列に 1 要素足すだけで済むようにしてある。
import { ReactNode, useState } from "react";
import { Member, PermGroup } from "../ipc";
import { t } from "../i18n";
import { SharedMemoView } from "./SharedMemoView";

type SharedHubProps = {
  configPath: string;
  isHost: boolean;
  /** 共有メモが使える状態か(member で false = ホスト未対応 or 未同期)。 */
  supported: boolean;
  /** 変更世代。進んだら再取得する。 */
  seq: number;
  members: Member[];
  /** 権限ダイアログで選べるグループ(ADR-0051)。host は既知の全グループ、member は自分の所属グループだけ。 */
  permGroups: PermGroup[];
  /** チャットの `@memo:id` カード(ADR-0052 決定 1)から開くメモ。 */
  focusMemoId?: string | null;
  onFocusConsumed?: () => void;
};

type SharedHubTab = {
  id: string;
  label: string;
  icon: string;
  render: (props: SharedHubProps) => ReactNode;
};

// サブタブを増やすときはここへ 1 要素足すだけ(id・ラベル・アイコン・
// render の配列駆動)。
const TABS: SharedHubTab[] = [
  {
    id: "memos",
    label: t.sharedHub.tabMemos,
    icon: "📝",
    render: (props) => <SharedMemoView {...props} />,
  },
];

export function SharedHubView(props: SharedHubProps) {
  const [tabId, setTabId] = useState(TABS[0].id);
  const active = TABS.find((tab) => tab.id === tabId) ?? TABS[0];

  return (
    <div className="shared-hub">
      <div className="shared-hub__tabs">
        {TABS.map((tab) => (
          <button
            key={tab.id}
            className={
              "shared-hub__tab" +
              (tab.id === active.id ? " shared-hub__tab--active" : "")
            }
            onClick={() => setTabId(tab.id)}
          >
            <span aria-hidden="true">{tab.icon}</span> {tab.label}
          </button>
        ))}
      </div>
      <div className="shared-hub__body">{active.render(props)}</div>
    </div>
  );
}

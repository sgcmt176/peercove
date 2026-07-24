// 共有ハブ(M5 F-5 Stage 1、ADR-0052 決定 3)。共有系機能をタブで増やし続け
// ず「共有」1 か所にまとめる器。内部はサブタブで切り替える。サブタブは
// 「メモ」「スケジュール」(M6 G-1、ADR-0053)。今後 Excel ライク表を足す
// ときも TABS 配列に 1 要素足すだけで済むようにしてある。
import { ReactNode, useEffect, useState } from "react";
import { Member, PermGroup } from "../ipc";
import { t } from "../i18n";
import { SharedMemoView } from "./SharedMemoView";
import { ScheduleView } from "./ScheduleView";

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
  /** チャットの `@schedule:id` カード(ADR-0053)から開く予定。 */
  focusScheduleId?: string | null;
  onScheduleFocusConsumed?: () => void;
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
  {
    id: "schedule",
    label: t.sharedHub.tabSchedule,
    icon: "📅",
    render: (props) => (
      <ScheduleView
        configPath={props.configPath}
        isHost={props.isHost}
        supported={props.supported}
        seq={props.seq}
        focusEventId={props.focusScheduleId}
        onFocusConsumed={props.onScheduleFocusConsumed}
      />
    ),
  },
];

export function SharedHubView(props: SharedHubProps) {
  const [tabId, setTabId] = useState(TABS[0].id);
  const active = TABS.find((tab) => tab.id === tabId) ?? TABS[0];

  // チャットの `@schedule:id` カードから開いたときは、反映前にサブタブを
  // 「スケジュール」へ切り替える(memo は元からこのタブがアクティブなので
  // 同様の配線は不要)。
  useEffect(() => {
    if (props.focusScheduleId) setTabId("schedule");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.focusScheduleId]);

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

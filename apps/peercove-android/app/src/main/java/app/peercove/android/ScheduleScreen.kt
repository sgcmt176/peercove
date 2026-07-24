package app.peercove.android

// 共有スケジュール表(M6 G-1、ADR-0053)。共有メモの基盤(ホスト正本 DB・
// コントロールチャネル配信・読み取りキャッシュ・世代ポーリング)を転用する。
// 閲覧・追加は全員、編集・削除は作成者 + ホストのみ(`canEdit` で判定)。
// 編集ロックは持たず revision CAS のみ(ADR-0053 決定 4)。終日予定は
// 開始 = その日のローカル 0 時、終了 = 終了日の 23:59:59.999(デスクトップ
// 実装と同じ規則)。
// **予定のタイトル・詳細は Log に出さないこと。**

import android.app.DatePickerDialog
import android.app.TimePickerDialog
import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Link
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringArrayResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale
import uniffi.peercove_mobile.MobileException
import uniffi.peercove_mobile.ScheduleEventInfo
import uniffi.peercove_mobile.scheduleCreate
import uniffi.peercove_mobile.scheduleDelete
import uniffi.peercove_mobile.scheduleList
import uniffi.peercove_mobile.scheduleUpdate
import uniffi.peercove_mobile.sharedMemoGeneration

private val scheduleDayFmt = SimpleDateFormat("M月d日(E)", Locale.JAPAN)
private val scheduleTimeFmt = SimpleDateFormat("HH:mm", Locale.JAPAN)
private val scheduleDateOnlyFmt = SimpleDateFormat("yyyy/MM/dd", Locale.JAPAN)
private val scheduleDateTimeFmt = SimpleDateFormat("yyyy/MM/dd HH:mm", Locale.JAPAN)
private const val DAY_MS = 24 * 60 * 60 * 1000L

// ---- 日付ユーティリティ(すべてローカル時刻扱い。デスクトップ実装と同じ規則) ----

private fun startOfDayMs(ms: Long): Long = Calendar.getInstance().apply {
    timeInMillis = ms
    set(Calendar.HOUR_OF_DAY, 0)
    set(Calendar.MINUTE, 0)
    set(Calendar.SECOND, 0)
    set(Calendar.MILLISECOND, 0)
}.timeInMillis

private fun endOfDayMs(ms: Long): Long = startOfDayMs(ms) + DAY_MS - 1

private fun startOfMonthMs(ms: Long): Long = Calendar.getInstance().apply {
    timeInMillis = startOfDayMs(ms)
    set(Calendar.DAY_OF_MONTH, 1)
}.timeInMillis

private fun addMonths(ms: Long, n: Int): Long = Calendar.getInstance().apply {
    timeInMillis = ms
    add(Calendar.MONTH, n)
}.timeInMillis

private fun addDays(ms: Long, n: Int): Long = Calendar.getInstance().apply {
    timeInMillis = ms
    add(Calendar.DATE, n)
}.timeInMillis

private fun isSameDay(a: Long, b: Long): Boolean = startOfDayMs(a) == startOfDayMs(b)

private fun monthOf(ms: Long): Int = Calendar.getInstance().apply { timeInMillis = ms }.get(Calendar.MONTH)

private fun dayOfMonth(ms: Long): Int =
    Calendar.getInstance().apply { timeInMillis = ms }.get(Calendar.DAY_OF_MONTH)

/** 日曜始まりの列インデックス(0=日 … 6=土)。Calendar.DAY_OF_WEEK は日曜=1。 */
private fun weekdayIndex(ms: Long): Int =
    Calendar.getInstance().apply { timeInMillis = ms }.get(Calendar.DAY_OF_WEEK) - 1

private fun monthLabel(ms: Long): String {
    val cal = Calendar.getInstance().apply { timeInMillis = ms }
    return "${cal.get(Calendar.YEAR)}年${cal.get(Calendar.MONTH) + 1}月"
}

private fun compareScheduleEvents(a: ScheduleEventInfo, b: ScheduleEventInfo): Int {
    if (a.allDay != b.allDay) return if (a.allDay) -1 else 1
    return a.startUnixMs.compareTo(b.startUnixMs)
}

private enum class ScheduleDialogMode { CREATE, EDIT }

private data class ScheduleDialogState(
    val mode: ScheduleDialogMode,
    val id: String? = null,
    val baseRevision: ULong = 0uL,
    val title: String = "",
    val note: String = "",
    val allDay: Boolean = false,
    val startMs: Long,
    val endMs: Long? = null,
)

/** 共有ハブの「スケジュール」サブタブ(SharedHubTabSpec から呼ばれる)。 */
@Composable
fun ScheduleTab(
    slug: String,
    onNotice: (String) -> Unit,
    /** チャットの `@schedule:id` カード(ADR-0053)から開く予定。 */
    focusEventId: String? = null,
    onFocusConsumed: () -> Unit = {},
) {
    val context = LocalContext.current
    val baseDir = context.filesDir.absolutePath
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current

    var month by remember { mutableStateOf(startOfMonthMs(System.currentTimeMillis())) }
    var selectedDay by remember { mutableStateOf<Long?>(startOfDayMs(System.currentTimeMillis())) }
    var events by remember { mutableStateOf<List<ScheduleEventInfo>>(emptyList()) }
    var offline by remember { mutableStateOf(false) }
    var supported by remember { mutableStateOf(true) }
    var loaded by remember { mutableStateOf(false) }
    var refreshTick by remember { mutableIntStateOf(0) }
    var selectedEvent by remember { mutableStateOf<ScheduleEventInfo?>(null) }
    var dialog by remember { mutableStateOf<ScheduleDialogState?>(null) }
    var confirmDeleteEvent by remember { mutableStateOf<ScheduleEventInfo?>(null) }
    var saving by remember { mutableStateOf(false) }

    val offlineMsg = stringResource(R.string.schedule_offline)
    val unsupportedMsg = stringResource(R.string.schedule_unsupported)
    val conflictMsg = stringResource(R.string.schedule_conflict_notice)
    val copiedFmt = stringResource(R.string.notice_copied)

    // チャットの `@schedule:id` カードから開く(一覧が届いてから 1 回だけ)
    LaunchedEffect(focusEventId, loaded) {
        val id = focusEventId ?: return@LaunchedEffect
        if (!loaded) return@LaunchedEffect
        events.firstOrNull { it.id == id }?.let { event ->
            val day = startOfDayMs(event.startUnixMs.toLong())
            month = startOfMonthMs(day)
            selectedDay = day
            selectedEvent = event
        }
        onFocusConsumed()
    }

    // 世代ポーリング(SharedMemoTab と同じ 2 秒流儀)。進んだら再取得する
    LaunchedEffect(Unit) {
        var lastGeneration = 0UL
        while (true) {
            val generation = withContext(Dispatchers.IO) { sharedMemoGeneration(slug) }
            if (generation != lastGeneration) {
                lastGeneration = generation
                refreshTick++
            }
            delay(2000)
        }
    }
    LaunchedEffect(refreshTick) {
        try {
            val result = withContext(Dispatchers.IO) { scheduleList(baseDir, slug) }
            events = result.events
            offline = result.offline
            supported = result.supported
        } catch (e: MobileException) {
            onNotice(e.message ?: "")
        }
        loaded = true
    }

    fun eventsForDay(day: Long): List<ScheduleEventInfo> {
        val dayStart = startOfDayMs(day)
        val dayEnd = dayStart + DAY_MS - 1
        return events.filter { e ->
            val end = e.endUnixMs?.toLong() ?: e.startUnixMs.toLong()
            e.startUnixMs.toLong() <= dayEnd && end >= dayStart
        }.sortedWith(::compareScheduleEvents)
    }

    fun openCreateDialog(day: Long) {
        val startMs = Calendar.getInstance().apply {
            timeInMillis = day
            set(Calendar.HOUR_OF_DAY, 9)
            set(Calendar.MINUTE, 0)
            set(Calendar.SECOND, 0)
            set(Calendar.MILLISECOND, 0)
        }.timeInMillis
        dialog = ScheduleDialogState(mode = ScheduleDialogMode.CREATE, startMs = startMs)
    }

    fun openEditDialog(event: ScheduleEventInfo) {
        dialog = ScheduleDialogState(
            mode = ScheduleDialogMode.EDIT,
            id = event.id,
            baseRevision = event.revision,
            title = event.title,
            note = event.note,
            allDay = event.allDay,
            startMs = event.startUnixMs.toLong(),
            endMs = event.endUnixMs?.toLong(),
        )
    }

    fun submit(state: ScheduleDialogState) {
        val title = state.title.trim()
        if (title.isBlank()) return
        val start = if (state.allDay) startOfDayMs(state.startMs) else state.startMs
        val end = state.endMs?.let { if (state.allDay) endOfDayMs(it) else it }
        saving = true
        scope.launch {
            try {
                val event = withContext(Dispatchers.IO) {
                    // 参加メンバーの選択 UI は次工程(M6 H-3、ADR-0055 決定 5)。
                    // ここでは Rust 層の新シグネチャに最小追随するだけで、
                    // 空リスト(参加メンバーなし)を渡す。
                    if (state.mode == ScheduleDialogMode.CREATE) {
                        scheduleCreate(
                            slug,
                            title,
                            state.note.trim(),
                            start.toULong(),
                            end?.toULong(),
                            state.allDay,
                            emptyList(),
                        )
                    } else {
                        scheduleUpdate(
                            slug,
                            state.id!!,
                            state.baseRevision,
                            title,
                            state.note.trim(),
                            start.toULong(),
                            end?.toULong(),
                            state.allDay,
                            emptyList(),
                        )
                    }
                }
                dialog = null
                val day = startOfDayMs(event.startUnixMs.toLong())
                selectedDay = day
                month = startOfMonthMs(day)
                refreshTick++
            } catch (e: MobileException) {
                val msg = e.message ?: ""
                if (msg.contains("competing_edit")) {
                    onNotice(conflictMsg)
                    dialog = null
                    refreshTick++
                } else {
                    onNotice(msg)
                }
            } finally {
                saving = false
            }
        }
    }

    fun copyLink(event: ScheduleEventInfo) {
        val token = sharedRefToken(SharedRefKind.SCHEDULE, event.id)
        clipboard.setText(AnnotatedString(token))
        onNotice(copiedFmt.format(token))
    }

    val readOnly = offline || !supported

    confirmDeleteEvent?.let { toDelete ->
        AlertDialog(
            onDismissRequest = { confirmDeleteEvent = null },
            title = { Text(stringResource(R.string.schedule_delete_event)) },
            text = { Text(stringResource(R.string.schedule_delete_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDeleteEvent = null
                    scope.launch {
                        try {
                            withContext(Dispatchers.IO) { scheduleDelete(slug, toDelete.id) }
                            selectedEvent = null
                            refreshTick++
                        } catch (e: MobileException) {
                            onNotice(e.message ?: "")
                        }
                    }
                }) { Text(stringResource(R.string.action_remove)) }
            },
            dismissButton = {
                TextButton(onClick = { confirmDeleteEvent = null }) {
                    Text(stringResource(R.string.action_cancel))
                }
            },
        )
    }

    dialog?.let { state ->
        ScheduleEditDialog(
            initial = state,
            saving = saving,
            onDismiss = { dialog = null },
            onSave = { submit(it) },
        )
    }

    selectedEvent?.let { event ->
        ScheduleDetailSheet(
            event = event,
            readOnly = readOnly,
            onDismiss = { selectedEvent = null },
            onEdit = {
                selectedEvent = null
                openEditDialog(event)
            },
            onDeleteRequested = { confirmDeleteEvent = event },
            onCopyLink = { copyLink(event) },
        )
    }

    Scaffold(
        floatingActionButton = {
            if (!readOnly) {
                FloatingActionButton(onClick = {
                    openCreateDialog(selectedDay ?: startOfDayMs(System.currentTimeMillis()))
                }) {
                    Icon(Icons.Filled.Add, contentDescription = stringResource(R.string.schedule_add_event))
                }
            }
        },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(horizontal = 12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                TextButton(onClick = { month = addMonths(month, -1) }) { Text("◀") }
                Text(
                    monthLabel(month),
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f),
                    textAlign = TextAlign.Center,
                )
                TextButton(onClick = { month = addMonths(month, 1) }) { Text("▶") }
                TextButton(onClick = {
                    val today = startOfDayMs(System.currentTimeMillis())
                    month = startOfMonthMs(today)
                    selectedDay = today
                }) { Text(stringResource(R.string.schedule_today)) }
            }
            if (offline) {
                Text(
                    offlineMsg,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
            } else if (!supported) {
                Text(
                    unsupportedMsg,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
            }
            Row(Modifier.fillMaxWidth()) {
                stringArrayResource(R.array.schedule_weekdays).forEach { label ->
                    Text(
                        label,
                        modifier = Modifier.weight(1f),
                        textAlign = TextAlign.Center,
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            val gridDays = remember(month) {
                val startOffset = weekdayIndex(month)
                val gridStart = addDays(month, -startOffset)
                List(42) { addDays(gridStart, it) }
            }
            for (row in 0 until 6) {
                Row(Modifier.fillMaxWidth()) {
                    for (col in 0 until 7) {
                        val day = gridDays[row * 7 + col]
                        val dayEvents = eventsForDay(day)
                        val sel = selectedDay
                        ScheduleCell(
                            dayNumber = dayOfMonth(day),
                            inMonth = monthOf(day) == monthOf(month),
                            isToday = isSameDay(day, System.currentTimeMillis()),
                            isSelected = sel != null && isSameDay(day, sel),
                            eventCount = dayEvents.size,
                            onClick = { selectedDay = day },
                        )
                    }
                }
            }
            Spacer(modifier = Modifier.height(8.dp))
            HorizontalDivider()
            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
            ) {
                Text(
                    selectedDay?.let { scheduleDayFmt.format(Date(it)) } ?: "",
                    style = MaterialTheme.typography.titleSmall,
                    modifier = Modifier.weight(1f),
                )
            }
            val dayEvents = selectedDay?.let { eventsForDay(it) } ?: emptyList()
            if (dayEvents.isEmpty()) {
                Text(
                    stringResource(R.string.schedule_no_events_for_day),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(vertical = 8.dp),
                )
            }
            dayEvents.forEach { event ->
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable { selectedEvent = event }
                        .padding(vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        if (event.allDay) {
                            stringResource(R.string.schedule_all_day)
                        } else {
                            scheduleTimeFmt.format(Date(event.startUnixMs.toLong()))
                        },
                        style = MaterialTheme.typography.labelSmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.width(56.dp),
                    )
                    Text(
                        event.title.ifEmpty { stringResource(R.string.memo_untitled) },
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.weight(1f),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (!event.canEdit) {
                        Text(
                            stringResource(R.string.schedule_viewer_badge),
                            style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
                HorizontalDivider(color = MaterialTheme.colorScheme.surfaceVariant)
            }
            // FAB に隠れないための余白
            Spacer(modifier = Modifier.height(80.dp))
        }
    }
}

/** カレンダーのセル 1 つ(小さなドット + 件数で予定の有無を示す。詳細な
 * タイトル表示はスマホの幅では省く)。 */
@Composable
private fun RowScope.ScheduleCell(
    dayNumber: Int,
    inMonth: Boolean,
    isToday: Boolean,
    isSelected: Boolean,
    eventCount: Int,
    onClick: () -> Unit,
) {
    val bg = when {
        isSelected -> MaterialTheme.colorScheme.primaryContainer
        isToday -> MaterialTheme.colorScheme.secondaryContainer
        else -> Color.Transparent
    }
    val textColor = if (inMonth) {
        MaterialTheme.colorScheme.onSurface
    } else {
        MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.5f)
    }
    Column(
        modifier = Modifier
            .weight(1f)
            .height(52.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(bg)
            .clickable(onClick = onClick)
            .padding(vertical = 4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(dayNumber.toString(), style = MaterialTheme.typography.bodySmall, color = textColor)
        if (eventCount > 0) {
            Spacer(modifier = Modifier.height(2.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    modifier = Modifier
                        .size(5.dp)
                        .clip(CircleShape)
                        .background(MaterialTheme.colorScheme.primary),
                )
                Spacer(modifier = Modifier.width(2.dp))
                Text(
                    eventCount.toString(),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
        }
    }
}

/** 予定の日時表示("7月28日(火) 14:00 〜 15:00" / 終日は "7月28日(火) 終日")。 */
@Composable
private fun scheduleEventTimeLabel(event: ScheduleEventInfo): String {
    val allDayLabel = stringResource(R.string.schedule_all_day)
    val start = event.startUnixMs.toLong()
    val end = event.endUnixMs?.toLong()
    return if (event.allDay) {
        val startLabel = scheduleDayFmt.format(Date(start))
        if (end != null && !isSameDay(start, end)) {
            "$startLabel $allDayLabel 〜 ${scheduleDayFmt.format(Date(end))}"
        } else {
            "$startLabel $allDayLabel"
        }
    } else {
        val startLabel = "${scheduleDayFmt.format(Date(start))} ${scheduleTimeFmt.format(Date(start))}"
        if (end != null) "$startLabel 〜 ${scheduleTimeFmt.format(Date(end))}" else startLabel
    }
}

/** 予定の詳細シート。リンクをコピー・編集・削除(編集・削除は canEdit のみ)。 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ScheduleDetailSheet(
    event: ScheduleEventInfo,
    readOnly: Boolean,
    onDismiss: () -> Unit,
    onEdit: () -> Unit,
    onDeleteRequested: () -> Unit,
    onCopyLink: () -> Unit,
) {
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp)) {
            Text(
                event.title.ifEmpty { stringResource(R.string.memo_untitled) },
                style = MaterialTheme.typography.titleMedium,
            )
            Spacer(modifier = Modifier.height(4.dp))
            Text(
                scheduleEventTimeLabel(event),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (event.note.isNotBlank()) {
                Spacer(modifier = Modifier.height(8.dp))
                Text(event.note, style = MaterialTheme.typography.bodyMedium)
            }
            Spacer(modifier = Modifier.height(8.dp))
            Text(
                stringResource(
                    R.string.schedule_owner,
                    event.ownerName.ifEmpty { stringResource(R.string.shared_memo_host) },
                ),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (event.updatedBy.isNotEmpty()) {
                Text(
                    stringResource(R.string.schedule_updated_by, event.updatedBy),
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Spacer(modifier = Modifier.height(12.dp))
            Row {
                IconButton(onClick = onCopyLink) {
                    Icon(Icons.Filled.Link, contentDescription = stringResource(R.string.schedule_copy_link))
                }
                if (event.canEdit && !readOnly) {
                    IconButton(onClick = onEdit) {
                        Icon(Icons.Filled.Edit, contentDescription = stringResource(R.string.schedule_edit_event))
                    }
                    IconButton(onClick = onDeleteRequested) {
                        Icon(
                            Icons.Filled.Delete,
                            contentDescription = stringResource(R.string.schedule_delete_event),
                        )
                    }
                }
            }
            Spacer(modifier = Modifier.height(16.dp))
        }
    }
}

/** 追加/編集フォーム。終日スイッチで時刻入力を隠す。日時入力は
 * DatePickerDialog + TimePickerDialog の組(Reminder.kt の流儀に合わせる)。 */
@Composable
private fun ScheduleEditDialog(
    initial: ScheduleDialogState,
    saving: Boolean,
    onDismiss: () -> Unit,
    onSave: (ScheduleDialogState) -> Unit,
) {
    val context = LocalContext.current
    var title by remember { mutableStateOf(initial.title) }
    var note by remember { mutableStateOf(initial.note) }
    var allDay by remember { mutableStateOf(initial.allDay) }
    var startMs by remember { mutableStateOf(initial.startMs) }
    var endMs by remember { mutableStateOf(initial.endMs) }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = {
            Text(
                stringResource(
                    if (initial.mode == ScheduleDialogMode.CREATE) {
                        R.string.schedule_create_title
                    } else {
                        R.string.schedule_edit_title
                    },
                ),
            )
        },
        text = {
            Column(modifier = Modifier.verticalScroll(rememberScrollState())) {
                OutlinedTextField(
                    value = title,
                    onValueChange = { title = it },
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    label = { Text(stringResource(R.string.schedule_title_label)) },
                    placeholder = { Text(stringResource(R.string.schedule_title_placeholder)) },
                )
                Spacer(modifier = Modifier.height(8.dp))
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.schedule_all_day), modifier = Modifier.weight(1f))
                    Switch(checked = allDay, onCheckedChange = { allDay = it })
                }
                Spacer(modifier = Modifier.height(4.dp))
                Text(stringResource(R.string.schedule_start_label), style = MaterialTheme.typography.labelSmall)
                TextButton(onClick = {
                    if (allDay) {
                        pickScheduleDate(context, startMs) { startMs = it }
                    } else {
                        pickScheduleDateTime(context, startMs) { startMs = it }
                    }
                }) {
                    Text(
                        if (allDay) {
                            scheduleDateOnlyFmt.format(Date(startMs))
                        } else {
                            scheduleDateTimeFmt.format(Date(startMs))
                        },
                    )
                }
                Spacer(modifier = Modifier.height(4.dp))
                Text(stringResource(R.string.schedule_end_label), style = MaterialTheme.typography.labelSmall)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    TextButton(onClick = {
                        val base = endMs ?: startMs
                        if (allDay) {
                            pickScheduleDate(context, base) { endMs = it }
                        } else {
                            pickScheduleDateTime(context, base) { endMs = it }
                        }
                    }) {
                        Text(
                            endMs?.let {
                                if (allDay) {
                                    scheduleDateOnlyFmt.format(Date(it))
                                } else {
                                    scheduleDateTimeFmt.format(Date(it))
                                }
                            } ?: stringResource(R.string.schedule_end_unset),
                        )
                    }
                    if (endMs != null) {
                        IconButton(onClick = { endMs = null }) {
                            Icon(
                                Icons.Filled.Close,
                                contentDescription = stringResource(R.string.schedule_clear_end),
                            )
                        }
                    }
                }
                Spacer(modifier = Modifier.height(8.dp))
                OutlinedTextField(
                    value = note,
                    onValueChange = { note = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text(stringResource(R.string.schedule_note_label)) },
                    placeholder = { Text(stringResource(R.string.schedule_note_placeholder)) },
                )
            }
        },
        confirmButton = {
            TextButton(
                enabled = title.isNotBlank() && !saving,
                onClick = {
                    onSave(
                        initial.copy(
                            title = title,
                            note = note,
                            allDay = allDay,
                            startMs = startMs,
                            endMs = endMs,
                        ),
                    )
                },
            ) { Text(stringResource(R.string.action_save)) }
        },
        dismissButton = {
            TextButton(onClick = onDismiss) { Text(stringResource(R.string.action_cancel)) }
        },
    )
}

private fun pickScheduleDate(context: Context, initialMillis: Long, onPicked: (Long) -> Unit) {
    val initial = Calendar.getInstance().apply { timeInMillis = initialMillis }
    DatePickerDialog(
        context,
        { _, year, month, day ->
            val picked = Calendar.getInstance().apply {
                set(year, month, day, 0, 0, 0)
                set(Calendar.MILLISECOND, 0)
            }
            onPicked(picked.timeInMillis)
        },
        initial.get(Calendar.YEAR),
        initial.get(Calendar.MONTH),
        initial.get(Calendar.DAY_OF_MONTH),
    ).show()
}

private fun pickScheduleDateTime(context: Context, initialMillis: Long, onPicked: (Long) -> Unit) {
    val initial = Calendar.getInstance().apply { timeInMillis = initialMillis }
    DatePickerDialog(
        context,
        { _, year, month, day ->
            TimePickerDialog(
                context,
                { _, hour, minute ->
                    val picked = Calendar.getInstance().apply {
                        set(year, month, day, hour, minute, 0)
                        set(Calendar.MILLISECOND, 0)
                    }
                    onPicked(picked.timeInMillis)
                },
                initial.get(Calendar.HOUR_OF_DAY),
                initial.get(Calendar.MINUTE),
                true,
            ).show()
        },
        initial.get(Calendar.YEAR),
        initial.get(Calendar.MONTH),
        initial.get(Calendar.DAY_OF_MONTH),
    ).show()
}

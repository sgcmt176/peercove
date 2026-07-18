package app.peercove.android

import com.journeyapps.barcodescanner.CaptureActivity

/**
 * 縦向き固定の QR 読み取り画面。zxing の既定 CaptureActivity は横向きのため、
 * マニフェストで screenOrientation="portrait" を指定したこのクラスを使う。
 */
class PortraitCaptureActivity : CaptureActivity()

package club.suitsu31.kagari.simulator

import android.app.AlertDialog
import android.app.NativeActivity
import android.content.pm.ActivityInfo
import android.os.Build
import android.os.Bundle
import android.os.Process
import android.view.View
import android.view.WindowInsets
import android.view.WindowInsetsController
import android.view.WindowManager
import android.window.OnBackInvokedDispatcher
import java.io.File

/**
 * Runs one game in the `:game` process: the Rust runtime (`libkagari_android.so`) drives the
 * window through `android_main`. Rust calls [gameDir], [lockOrientation], [showFatalError] and
 * [finishGame] by name.
 */
class GameActivity : NativeActivity() {
    companion object {
        const val EXTRA_GAME_DIR = "club.suitsu31.kagari.simulator.GAME_DIR"

        /** The game directory of the running game, so the launcher can tell it apart. */
        const val RUNNING_FILE = "running-game.txt"

        init {
            // Registers the library for the JNI lookup of `nativeOnBack`; NativeActivity loads
            // it again, which is harmless.
            System.loadLibrary("kagari_android")
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        File(filesDir, RUNNING_FILE).writeText(gameDir())
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        window.attributes = window.attributes.apply {
            layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
        }
        hideSystemBars()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(
                OnBackInvokedDispatcher.PRIORITY_DEFAULT,
            ) { nativeOnBack() }
        }
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            hideSystemBars()
        }
    }

    /**
     * Before API 33 the back key reaches Rust as a key event; the activity must not finish. From
     * API 33 the `OnBackInvokedCallback` registered in [onCreate] handles back gestures instead.
     */
    @Deprecated("Superseded by OnBackInvokedCallback on API 33+")
    @Suppress("DEPRECATION", "GestureBackNavigation")
    override fun onBackPressed() {}

    override fun onPause() {
        super.onPause()
        // The launcher resumes before `onDestroy`: it must already see the game as ended.
        if (isFinishing) {
            clearRunning()
        }
    }

    override fun onDestroy() {
        // The event loop ends (autosaving the game) once told; `super.onDestroy` returns after
        // `android_main` has returned: the glue waits for the native thread.
        nativeOnDestroy()
        super.onDestroy()
        clearRunning()
        // winit allows one event loop per process: never host a second `android_main`.
        Process.killProcess(Process.myPid())
    }

    private fun clearRunning() {
        val running = File(filesDir, RUNNING_FILE)
        if (running.exists() && running.readText() == gameDir()) {
            running.delete()
        }
    }

    @Suppress("DEPRECATION")
    private fun hideSystemBars() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
            window.insetsController?.let {
                it.hide(WindowInsets.Type.systemBars())
                it.systemBarsBehavior = WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            }
        } else {
            window.decorView.systemUiVisibility = View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY or
                View.SYSTEM_UI_FLAG_FULLSCREEN or
                View.SYSTEM_UI_FLAG_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or
                View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION or
                View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
        }
    }

    fun gameDir(): String = intent.getStringExtra(EXTRA_GAME_DIR) ?: ""

    fun lockOrientation(landscape: Boolean) {
        runOnUiThread {
            requestedOrientation = if (landscape) {
                ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
            } else {
                ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT
            }
        }
    }

    fun showFatalError(message: String) {
        runOnUiThread {
            if (!isFinishing) {
                AlertDialog.Builder(this)
                    .setTitle(R.string.game_failed_title)
                    .setMessage(message)
                    .setCancelable(false)
                    .setPositiveButton(android.R.string.ok) { _, _ -> finish() }
                    .show()
            }
        }
    }

    fun finishGame() {
        runOnUiThread { finish() }
    }

    private external fun nativeOnBack()

    private external fun nativeOnDestroy()
}

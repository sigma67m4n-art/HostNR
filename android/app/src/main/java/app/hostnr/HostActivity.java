package app.hostnr;

import android.Manifest;
import android.app.NativeActivity;
import android.content.Intent;
import android.content.res.Configuration;
import android.database.Cursor;
import android.net.Uri;
import android.net.VpnService;
import android.os.Build;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import android.text.Editable;
import android.text.InputType;
import android.text.TextWatcher;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.view.inputmethod.InputMethodManager;
import android.widget.EditText;

/* Android lifecycle, document, palette, and keyboard adapters only. */
public final class HostActivity extends NativeActivity {

	private static final int VPN = 10;
	private static final int IMPORT = 11;
	private static final int EXPORT = 12;
	private EditText input;
	private int inputField;
	private boolean syncing;
	private volatile int topInset;
	private volatile int bottomInset;

	static {
		System.loadLibrary("hostnr");
	}

	private static native void nativeFile(int fd, String name);
	private static native void nativeExport(int fd);
	public static native void nativeMessage(String message);
	private static native void nativeText(int field, String text);

	@Override
	protected void onCreate(Bundle state) {
		super.onCreate(state);
		View decor = getWindow().getDecorView();
		decor.setOnApplyWindowInsetsListener((view, insets) -> {
			float density = getResources().getDisplayMetrics().density;
			if (Build.VERSION.SDK_INT >= 30) {
				android.graphics.Insets bars = insets.getInsets(
					WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
				topInset = Math.round(bars.top / density);
				bottomInset = Math.round(bars.bottom / density);
			} else {
				topInset = Math.round(insets.getSystemWindowInsetTop() / density);
				bottomInset = Math.round(insets.getSystemWindowInsetBottom() / density);
			}
			return insets;
		});
		decor.requestApplyInsets();

		/* Keep the IME adapter invisible; all visible controls are drawn in Rust. */
		input = new EditText(this);
		input.setSingleLine(true);
		input.setAlpha(0f);
		input.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
		input.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
		input.addTextChangedListener(new TextWatcher() {
			@Override
			public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
			@Override
			public void onTextChanged(CharSequence s, int start, int before, int count) {
				if (!syncing)
					nativeText(inputField, s.toString());
			}
			@Override
			public void afterTextChanged(Editable s) {}
		});
		ViewGroup content = findViewById(android.R.id.content);
		content.addView(input, new ViewGroup.LayoutParams(1, 1));
	}

	public void editText(int field, String text) {
		runOnUiThread(() -> {
			inputField = field;
			syncing = true;
			input.setText(text);
			input.setSelection(text.length());
			syncing = false;
			input.requestFocus();
			InputMethodManager manager = getSystemService(InputMethodManager.class);
			manager.showSoftInput(input, InputMethodManager.SHOW_IMPLICIT);
		});
	}

	public void finishText() {
		runOnUiThread(() -> {
			InputMethodManager manager = getSystemService(InputMethodManager.class);
			manager.hideSoftInputFromWindow(input.getWindowToken(), 0);
			input.clearFocus();
		});
	}

	public void requestVpn() {
		runOnUiThread(() -> {
			try {
				Intent permission = VpnService.prepare(this);
				if (permission == null)
					startVpn();
				else
					startActivityForResult(permission, VPN);
			} catch (Exception e) {
				nativeMessage("Cannot request VPN permission");
			}
		});
	}

	private void startVpn() {
		try {
			startForegroundService(new Intent(this, HostVpnService.class));
			if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
				!= android.content.pm.PackageManager.PERMISSION_GRANTED)
				requestPermissions(new String[] {Manifest.permission.POST_NOTIFICATIONS}, 20);
		} catch (Exception e) {
			nativeMessage("Cannot start VPN: " + e.getMessage());
		}
	}

	public void stopVpn() {
		runOnUiThread(() -> stopService(new Intent(this, HostVpnService.class)));
	}

	public void pickHosts() {
		runOnUiThread(() -> {
			try {
				Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
				intent.addCategory(Intent.CATEGORY_OPENABLE);
				intent.setType("*/*");
				startActivityForResult(intent, IMPORT);
			} catch (Exception e) {
				nativeMessage("No file picker available");
			}
		});
	}

	public void exportHosts() {
		runOnUiThread(() -> {
			try {
				Intent intent = new Intent(Intent.ACTION_CREATE_DOCUMENT);
				intent.addCategory(Intent.CATEGORY_OPENABLE);
				intent.setType("text/plain");
				intent.putExtra(Intent.EXTRA_TITLE, "HostNR.hosts");
				startActivityForResult(intent, EXPORT);
			} catch (Exception e) {
				nativeMessage("No file picker available");
			}
		});
	}

	@Override
	protected void onActivityResult(int request, int result, Intent data) {
		super.onActivityResult(request, result, data);
		if (request == VPN) {
			if (result == RESULT_OK)
				startVpn();
			else
				nativeMessage("VPN permission denied");
			return;
		}
		if (result != RESULT_OK || data == null || data.getData() == null)
			return;
		Uri uri = data.getData();
		try (ParcelFileDescriptor descriptor = getContentResolver().openFileDescriptor(
			uri, request == IMPORT ? "r" : "wt")) {
			if (descriptor == null)
				throw new IllegalStateException("Cannot open document");
			if (request == IMPORT) {
				String name = "Imported hosts";
				try (Cursor cursor = getContentResolver().query(uri,
					new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
					if (cursor != null && cursor.moveToFirst())
						name = cursor.getString(0);
				}
				nativeFile(descriptor.detachFd(), name);
			} else if (request == EXPORT) {
				nativeExport(descriptor.detachFd());
			}
		} catch (Exception e) {
			nativeMessage("Cannot open document: " + e.getMessage());
		}
	}

	public int[] palette() {
		boolean dark = (getResources().getConfiguration().uiMode & Configuration.UI_MODE_NIGHT_MASK)
			== Configuration.UI_MODE_NIGHT_YES;
		int[] colors = new int[12];
		colors[0] = dark ? 1 : 0;
		colors[10] = topInset;
		colors[11] = bottomInset;
		if (Build.VERSION.SDK_INT >= 31) {
			colors[1] = getColor(dark ? android.R.color.system_accent1_200 : android.R.color.system_accent1_600);
			colors[2] = dark ? getColor(android.R.color.system_accent1_800) : 0xffffffff;
			colors[3] = getColor(dark ? android.R.color.system_accent1_700 : android.R.color.system_accent1_100);
			colors[4] = getColor(dark ? android.R.color.system_accent1_100 : android.R.color.system_accent1_900);
			colors[5] = getColor(dark ? android.R.color.system_neutral1_900 : android.R.color.system_neutral1_10);
			colors[6] = getColor(dark ? android.R.color.system_neutral1_100 : android.R.color.system_neutral1_900);
			colors[7] = getColor(dark ? android.R.color.system_neutral1_800 : android.R.color.system_neutral1_50);
			colors[8] = getColor(dark ? android.R.color.system_accent2_200 : android.R.color.system_accent2_600);
			colors[9] = 1;
		}
		return colors;
	}
}

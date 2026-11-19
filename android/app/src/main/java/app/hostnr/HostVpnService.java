package app.hostnr;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.net.VpnService;
import android.os.Build;
import android.os.ParcelFileDescriptor;

/* Android owns service lifecycle; Rust owns TUN and DNS processing. */
public final class HostVpnService extends VpnService {

	private static final String CHANNEL = "hostnr_vpn";
	private Thread worker;
	private boolean stopping;

	static {
		System.loadLibrary("hostnr");
	}

	private static native long nativePrepare();
	private static native void nativeRun(int fd, long token, HostVpnService service);
	private static native void nativeStop();

	@Override
	public int onStartCommand(Intent intent, int flags, int startId) {
		if (intent != null && "STOP".equals(intent.getAction())) {
			stopSelf();
			return START_NOT_STICKY;
		}
		if (worker != null && worker.isAlive())
			return START_NOT_STICKY;
		/*
		 * User-approved VPN apps qualify for systemExempted without alarm
		 * permissions. Lint cannot model this runtime eligibility, so the
		 * manifest suppresses ForegroundServicePermission for this service only.
		 * https://developer.android.com/develop/background-work/services/fgs/service-types#system-exempted
		 */
		if (VpnService.prepare(this) != null) {
			HostActivity.nativeMessage("VPN permission is missing");
			stopSelf();
			return START_NOT_STICKY;
		}
		try {
			NotificationManager manager = getSystemService(NotificationManager.class);
			manager.createNotificationChannel(new NotificationChannel(
				CHANNEL, "Local DNS", NotificationManager.IMPORTANCE_LOW));
			PendingIntent open = PendingIntent.getActivity(this, 0,
				new Intent(this, HostActivity.class), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
			PendingIntent stop = PendingIntent.getService(this, 1,
				new Intent(this, HostVpnService.class).setAction("STOP"), PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
			Notification notification = new Notification.Builder(this, CHANNEL)
				.setSmallIcon(R.drawable.ic_notification)
				.setContentTitle("HostNR")
				.setContentText("Hosts active")
				.setContentIntent(open)
				.setOngoing(true)
				.addAction(new Notification.Action.Builder(null, "Disconnect", stop).build())
				.build();
			if (Build.VERSION.SDK_INT >= 34)
				startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SYSTEM_EXEMPTED);
			else
				startForeground(1, notification);

			ParcelFileDescriptor tun = new Builder()
				.setSession("HostNR")
				.setConfigureIntent(open)
				.setMtu(1500)
				.addAddress("10.77.0.1", 32)
				.addRoute("10.77.0.2", 32)
				.addDnsServer("10.77.0.2")
				.allowFamily(android.system.OsConstants.AF_INET6)
				.setBlocking(true)
				.establish();
			if (tun == null)
				throw new IllegalStateException("VPN permission is missing");
			int fd = tun.detachFd();
			tun.close();
			stopping = false;
			long token = nativePrepare();
			worker = new Thread(() -> {
				try {
					nativeRun(fd, token, this);
				} finally {
					getMainExecutorCompat().execute(() -> {
						if (!stopping)
							stopSelf();
					});
				}
			}, "HostNR-DNS");
			worker.start();
		} catch (Exception e) {
			HostActivity.nativeMessage("VPN failed: " + e.getMessage());
			stopSelf();
		}
		return START_NOT_STICKY;
	}

	private java.util.concurrent.Executor getMainExecutorCompat() {
		android.os.Handler handler = new android.os.Handler(getMainLooper());
		return command -> handler.post(command);
	}

	@Override
	public void onRevoke() {
		nativeStop();
		stopSelf();
		super.onRevoke();
	}

	@Override
	public void onDestroy() {
		stopping = true;
		nativeStop();
		stopForeground(STOP_FOREGROUND_REMOVE);
		super.onDestroy();
	}
}

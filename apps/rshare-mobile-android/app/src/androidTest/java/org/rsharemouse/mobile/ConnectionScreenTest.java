package org.rsharemouse.mobile;

import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;

import android.content.Context;
import androidx.test.platform.app.InstrumentationRegistry;
import androidx.test.uiautomator.By;
import androidx.test.uiautomator.UiDevice;
import androidx.test.uiautomator.Until;
import org.junit.Test;

public final class ConnectionScreenTest {
    @Test
    public void discoveryScreenShowsComputerListAndRevealsManualConnection() throws Exception {
        Context context = InstrumentationRegistry.getInstrumentation().getTargetContext();
        UiDevice device = UiDevice.getInstance(InstrumentationRegistry.getInstrumentation());
        context.getSharedPreferences("mobile_connection", Context.MODE_PRIVATE).edit().clear().commit();
        device.executeShellCommand("am start -n " + context.getPackageName()
                + "/.MainActivity -f 0x10008000");
        assertNotNull(device.wait(Until.findObject(By.res(context.getPackageName(), "discovery_title")), 15000));
        assertNotNull(device.wait(Until.findObject(By.res(context.getPackageName(), "discovery_status")), 15000));
        assertNull(device.findObject(By.res(context.getPackageName(), "url_input")));
        device.wait(Until.findObject(By.res(context.getPackageName(), "manual_toggle")), 15000).click();
        assertNotNull(device.wait(Until.findObject(By.res(context.getPackageName(), "url_input")), 5000));
        assertNotNull(device.wait(Until.findObject(By.res(context.getPackageName(), "scan_button")), 5000));
    }
}

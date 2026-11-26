import asyncio
import sys
from pathlib import Path

import decky

MODULES = Path(__file__).resolve().parent / "py_modules"
if str(MODULES) not in sys.path:
    sys.path.insert(0, str(MODULES))

from audiobridge.backend import Backend, UserError  # noqa: E402

# Decky calls the methods below on the class itself, so shared state lives at
# module level rather than on an instance.
_backend = None
_autostart_task = None


def backend():
    global _backend
    if _backend is None:
        _backend = Backend(decky.DECKY_PLUGIN_SETTINGS_DIR, str(MODULES))
    return _backend


async def autostart():
    # Right after boot Decky may load before the user session is up, so keep
    # trying for a couple of minutes before giving up.
    for _ in range(40):
        try:
            result = await asyncio.to_thread(backend().autostart)
        except Exception:
            decky.logger.exception("Audio Bridge: automatic start failed")
            return
        if result is True:
            decky.logger.info("Audio Bridge: streaming started automatically")
        if result is not None:
            return
        await asyncio.sleep(3)
    decky.logger.warning("Audio Bridge: the user session did not come up, automatic start skipped")


class Plugin:
    async def _main(self):
        global _autostart_task
        backend()
        # Return at once; the automatic start may have to wait for the session.
        _autostart_task = asyncio.get_running_loop().create_task(autostart())

    async def _unload(self):
        # Never await here: the loader stops serving this event loop right
        # after asking the plugin to stop. The stream itself keeps running in
        # its own user unit, so a reload or update does not interrupt it.
        if _autostart_task is not None:
            _autostart_task.cancel()
        decky.logger.info("Audio Bridge: panel backend unloaded, stream left as it was")

    async def _uninstall(self):
        try:
            backend().uninstall()
        except Exception:
            decky.logger.exception("Audio Bridge: could not stop the engine during uninstall")

    async def _migration(self):
        pass

    async def request(self, method, args=None):
        b = backend()
        routes = {
            "state": b.state,
            "start": b.start,
            "stop": b.stop,
            "settings": b.update_settings,
            "set_address": b.set_address,
            "choose_receiver": b.choose_receiver,
            "discover": b.discover,
            "test_sound": b.test_sound,
            "diagnostics": b.diagnostics,
        }
        handler = routes.get(method)
        if handler is None:
            return {"ok": False, "error": "Unknown request."}
        try:
            if isinstance(args, dict):
                result = await asyncio.to_thread(handler, **args)
            elif args is None:
                result = await asyncio.to_thread(handler)
            else:
                return {"ok": False, "error": "Bad request."}
            return {"ok": True, "data": result}
        except UserError as exc:
            return {"ok": False, "error": str(exc)}
        except TypeError:
            decky.logger.exception("Audio Bridge: bad arguments for %s", method)
            return {"ok": False, "error": "Bad request."}
        except Exception as exc:
            decky.logger.exception("Audio Bridge: %s failed", method)
            return {"ok": False, "error": "Something went wrong: %s" % exc}

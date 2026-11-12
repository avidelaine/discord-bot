"""PC control for Bobby: media sessions (SMTC) and master volume (Core Audio). Included by server.py."""
from fastapi import APIRouter, HTTPException
from winrt.windows.media.control import GlobalSystemMediaTransportControlsSessionManager as Manager

router = APIRouter(prefix="/pc")
PREFERRED = ("brave", "chrome", "music", "spotify")  # ponytail: first match wins, else the current session


async def session():
    mgr = await Manager.request_async()
    sessions = list(mgr.get_sessions())
    for key in PREFERRED:
        for s in sessions:
            if key in s.source_app_user_model_id.lower():
                return s
    return mgr.get_current_session()


async def now_playing():
    s = await session()
    if not s:
        return {"playing": False, "title": "", "artist": ""}
    info = await s.try_get_media_properties_async()
    return {"playing": s.get_playback_info().playback_status == 4, "title": info.title, "artist": info.artist}


@router.get("/media")
async def media_get():
    return await now_playing()


@router.post("/media/{cmd}")
async def media_cmd(cmd: str):
    s = await session()
    if not s:
        raise HTTPException(404, "no media session")
    ok = await {
        "play": s.try_play_async, "pause": s.try_pause_async, "toggle": s.try_toggle_play_pause_async,
        "next": s.try_skip_next_async, "previous": s.try_skip_previous_async,
    }[cmd]()
    return {"ok": bool(ok), **await now_playing()}


def _vol():
    from pycaw.pycaw import AudioUtilities
    dev = AudioUtilities.GetSpeakers()
    return getattr(dev, "EndpointVolume", None) or _legacy(dev)


def _legacy(dev):
    from comtypes import CLSCTX_ALL
    from pycaw.pycaw import IAudioEndpointVolume
    return dev.Activate(IAudioEndpointVolume._iid_, CLSCTX_ALL, None).QueryInterface(IAudioEndpointVolume)


def volume_state(v=None):
    v = v or _vol()
    return {"level": round(v.GetMasterVolumeLevelScalar() * 100), "muted": bool(v.GetMute())}


@router.get("/volume")
def volume_get():
    return volume_state()


@router.post("/volume")
def volume_set(level: int | None = None, delta: int | None = None, mute: bool | None = None):
    v = _vol()
    if level is not None or delta is not None:
        new = level if level is not None else round(v.GetMasterVolumeLevelScalar() * 100) + delta
        v.SetMasterVolumeLevelScalar(max(0, min(100, new)) / 100, None)
    if mute is not None:
        v.SetMute(int(mute), None)
    return volume_state(v)

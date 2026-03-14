// APRS Track Playback Engine
// Animates through track points chronologically with speed control
(function() {
    'use strict';

    var points = [];       // Merged sorted points from getTrackData()
    var index = 0;
    var playing = false;
    var speed = 5;         // Multiplier
    var timer = null;
    var tickIntervalMs = 100;  // Base tick rate

    function startPlayback() {
        if (typeof window.getTrackData !== 'function') return;
        points = window.getTrackData();
        if (points.length === 0) return;

        index = 0;
        playing = true;
        speed = parseInt(document.getElementById('playback-speed').value) || 5;

        var bar = document.getElementById('playback-bar');
        var slider = document.getElementById('playback-slider');
        if (bar) bar.style.display = 'flex';
        if (slider) {
            slider.max = points.length - 1;
            slider.value = 0;
        }

        updateToggleBtn();
        tick();
        scheduleNext();
    }

    function stopPlayback() {
        playing = false;
        if (timer) { clearTimeout(timer); timer = null; }
        var bar = document.getElementById('playback-bar');
        if (bar) bar.style.display = 'none';
        if (typeof updatePlaybackMarker === 'function') {
            updatePlaybackMarker(null, null);
        }
    }

    function togglePlayback() {
        if (playing) {
            playing = false;
            if (timer) { clearTimeout(timer); timer = null; }
        } else {
            if (index >= points.length - 1) index = 0;
            playing = true;
            scheduleNext();
        }
        updateToggleBtn();
    }

    function updateToggleBtn() {
        var btn = document.getElementById('playback-toggle');
        if (btn) btn.innerHTML = playing ? '&#x23F8;' : '&#x25B6;';
    }

    function scheduleNext() {
        if (!playing || index >= points.length - 1) {
            if (index >= points.length - 1) {
                playing = false;
                updateToggleBtn();
            }
            return;
        }

        // Calculate delay based on time gap between points and speed multiplier
        var gap = 0;
        if (index < points.length - 1) {
            gap = points[index + 1].timeMs - points[index].timeMs;
        }
        // Scale gap by speed, but clamp to reasonable range
        var delay = Math.max(20, Math.min(2000, gap / (speed * 1000)));

        timer = setTimeout(function() {
            index++;
            tick();
            scheduleNext();
        }, delay);
    }

    function tick() {
        if (index >= points.length) return;
        var p = points[index];

        // Update marker position on map
        if (typeof updatePlaybackMarker === 'function') {
            updatePlaybackMarker(p.lon, p.lat);
        }

        // Update slider
        var slider = document.getElementById('playback-slider');
        if (slider) slider.value = index;

        // Update time display
        var timeEl = document.getElementById('playback-time');
        if (timeEl && p.time) {
            var ts = p.time.replace(' ', 'T');
            var d = new Date(ts.endsWith('Z') ? ts : ts + 'Z');
            if (!isNaN(d)) {
                timeEl.textContent = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', timeZone: 'UTC' }) + ' UTC';
            }
        }
    }

    function seekTo(idx) {
        index = Math.max(0, Math.min(points.length - 1, idx));
        tick();
    }

    function setSpeed(mult) {
        speed = mult;
    }

    // Expose globally
    window.startPlayback = startPlayback;

    // Initialize controls when DOM is ready
    function initPlaybackControls() {
        var toggle = document.getElementById('playback-toggle');
        var slider = document.getElementById('playback-slider');
        var speedSel = document.getElementById('playback-speed');
        var closeBtn = document.getElementById('playback-close');

        if (toggle) {
            toggle.addEventListener('click', togglePlayback);
        }
        if (slider) {
            slider.addEventListener('input', function() {
                seekTo(parseInt(slider.value));
            });
        }
        if (speedSel) {
            speedSel.addEventListener('change', function() {
                speed = parseInt(speedSel.value) || 5;
            });
        }
        if (closeBtn) {
            closeBtn.addEventListener('click', stopPlayback);
        }
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', initPlaybackControls);
    } else {
        initPlaybackControls();
    }
})();

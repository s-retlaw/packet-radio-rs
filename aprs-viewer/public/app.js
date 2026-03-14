// APRS Viewer — Main Application
// Vanilla JS frontend: WebSocket client, state management, DOM rendering

(function() {
    'use strict';

    // ===== Track Colors =====
    const TRACK_COLORS = ['#4fc3f7','#f06292','#aed581','#ffb74d','#ba68c8',
                           '#4dd0e1','#e57373','#81c784','#ffd54f','#7986cb'];

    // ===== State =====
    const state = {
        stations: new Map(),    // callsign -> StationRow
        packets: [],            // PacketRow[], newest first
        selectedStation: null,  // callsign string or null
        connected: false,
        searchFilter: '',
        sourceFilter: 'all',   // "all", "tnc", or "aprs-is"
        tracks: new Map(),      // callsign -> { points, color, visible, hours, cachedPoints }
        trackColorIndex: 0,
        trackPanelOpen: false,
        trackTimeStart: 24,    // hours ago (slider left)
        trackTimeEnd: 0,       // hours ago (slider right, 0 = now)
        selectedTrack: null,   // callsign of selected track for stats
        ws: null,
        reconnectTimer: null,
        maxPackets: 500,
    };

    // ===== DOM References =====
    const dom = {
        stationTbody: document.getElementById('station-tbody'),
        stationCount: document.getElementById('station-count'),
        stationDetail: document.getElementById('station-detail'),
        packetTbody: document.getElementById('packet-tbody'),
        packetCount: document.getElementById('packet-count'),
        searchInput: document.getElementById('search-input'),
        showFccNearby: document.getElementById('show-fcc-nearby'),
        statusDot: document.getElementById('status-dot'),
        statusText: document.getElementById('status-text'),
        btnSettings: document.getElementById('btn-settings'),
        btnTracks: document.getElementById('btn-tracks'),
        trackPanel: document.getElementById('track-panel'),
        settingsModal: document.getElementById('settings-modal'),
        settingsClose: document.getElementById('settings-close'),
        settingsCancel: document.getElementById('settings-cancel'),
        settingsSave: document.getElementById('settings-save'),
        settingsStatus: document.getElementById('settings-status'),
    };

    // ===== WebSocket =====
    function connectWebSocket() {
        if (state.ws && state.ws.readyState <= 1) return;

        const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
        const url = `${proto}//${location.host}/ws/packets`;

        state.ws = new WebSocket(url);

        state.ws.onopen = () => {
            setConnected(true);
            if (state.reconnectTimer) {
                clearTimeout(state.reconnectTimer);
                state.reconnectTimer = null;
            }
        };

        state.ws.onmessage = (e) => {
            try {
                const event = JSON.parse(e.data);
                handleWsEvent(event);
            } catch (err) {
                console.error('WS parse error:', err);
            }
        };

        state.ws.onclose = () => {
            setConnected(false);
            scheduleReconnect();
        };

        state.ws.onerror = () => {
            state.ws.close();
        };
    }

    function scheduleReconnect() {
        if (state.reconnectTimer) return;
        state.reconnectTimer = setTimeout(() => {
            state.reconnectTimer = null;
            connectWebSocket();
        }, 3000);
    }

    function setConnected(connected) {
        state.connected = connected;
        dom.statusDot.className = 'status-dot ' + (connected ? 'connected' : 'disconnected');
        dom.statusText.textContent = connected ? 'Connected' : 'Disconnected';
    }

    // ===== WS Event Handlers =====
    function handleWsEvent(event) {
        switch (event.type) {
            case 'Init':
                state.packets = event.packets || [];
                renderPackets();
                fetchStations();
                break;
            case 'Packet':
                addPacket(event);
                break;
            case 'StationUpdate':
                updateStation(event);
                break;
        }
    }

    function addPacket(pkt) {
        state.packets.unshift(pkt);
        if (state.packets.length > state.maxPackets) {
            state.packets.length = state.maxPackets;
        }
        renderPackets();
    }

    function updateStation(station) {
        const key = stationKey(station);
        state.stations.set(key, station);
        renderStations();
        updateMapStations();

        if (state.selectedStation === key) {
            renderStationDetail(station);
        }

        if (typeof pulseStation === 'function') {
            pulseStation(station);
        }
    }

    // ===== REST API =====
    async function fetchStations() {
        try {
            const resp = await fetch('/api/stations');
            if (!resp.ok) return;
            const stations = await resp.json();
            state.stations.clear();
            for (const s of stations) {
                state.stations.set(stationKey(s), s);
            }
            renderStations();
            updateMapStations();
        } catch (e) {
            console.error('Failed to fetch stations:', e);
        }
    }

    async function fetchStationTrack(callsign, ssid, hours) {
        try {
            const call = ssid > 0 ? `${callsign}-${ssid}` : callsign;
            const h = hours || 48;
            const resp = await fetch(`/api/stations/${encodeURIComponent(call)}/track?hours=${h}`);
            if (!resp.ok) return [];
            return await resp.json();
        } catch (e) {
            console.error('Failed to fetch track:', e);
            return [];
        }
    }

    async function fetchConfig() {
        try {
            const resp = await fetch('/api/config');
            if (!resp.ok) return null;
            return await resp.json();
        } catch (e) {
            console.error('Failed to fetch config:', e);
            return null;
        }
    }

    async function saveConfig(config) {
        const resp = await fetch('/api/config', {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(config),
        });
        if (!resp.ok) {
            const text = await resp.text();
            throw new Error(text || 'Save failed');
        }
    }

    // ===== Source Filter =====
    function matchesSourceFilter(station) {
        if (state.sourceFilter === 'all') return true;
        return station.heard_via && station.heard_via.indexOf(state.sourceFilter) >= 0;
    }

    function matchesPacketSourceFilter(pkt) {
        if (state.sourceFilter === 'all') return true;
        return pkt.source_type === state.sourceFilter;
    }

    // ===== Multi-Track Management =====

    async function addTrack(callsign, ssid, hours) {
        const call = ssid > 0 ? `${callsign}-${ssid}` : callsign;
        if (state.tracks.has(call)) return call;

        const color = TRACK_COLORS[state.trackColorIndex % TRACK_COLORS.length];
        state.trackColorIndex++;

        const trackEntry = {
            callsign: callsign,
            ssid: ssid,
            color: color,
            visible: true,
            hours: hours || 48,
            cachedPoints: null,
            points: [],
        };

        state.tracks.set(call, trackEntry);

        // Fetch full 48h of data and cache
        const points = await fetchStationTrack(callsign, ssid, 48);
        trackEntry.cachedPoints = points;
        trackEntry.points = filterTrackByTime(points, state.trackTimeStart, state.trackTimeEnd);

        rebuildAllTrackSources();
        updateTrackButton();
        renderTrackPanel();

        if (!state.trackPanelOpen) {
            toggleTrackPanel(true);
        }
    }

    function removeTrack(call) {
        state.tracks.delete(call);
        if (state.selectedTrack === call) state.selectedTrack = null;
        rebuildAllTrackSources();
        updateTrackButton();
        renderTrackPanel();
        if (state.tracks.size === 0) {
            toggleTrackPanel(false);
        }
    }

    async function trackAllMoved() {
        var moved = [];
        for (var entry of state.stations) {
            var s = entry[1];
            if (s.has_moved && s.lat != null && !isTracked(stationKey(s))) {
                moved.push(s);
            }
        }
        for (var i = 0; i < moved.length; i++) {
            addTrack(moved[i].callsign, moved[i].ssid, 48);
            if (i < moved.length - 1) {
                await new Promise(function(r) { setTimeout(r, 50); });
            }
        }
    }

    function clearAllTracks() {
        state.tracks.clear();
        state.selectedTrack = null;
        rebuildAllTrackSources();
        updateTrackButton();
        renderTrackPanel();
    }

    function toggleTrackVisibility(call) {
        const t = state.tracks.get(call);
        if (!t) return;
        t.visible = !t.visible;
        rebuildAllTrackSources();
        renderTrackPanel();
    }

    function isTracked(call) {
        return state.tracks.has(call);
    }

    function filterTrackByTime(points, startHoursAgo, endHoursAgo) {
        if (!points || points.length === 0) return [];
        const now = Date.now();
        const startMs = now - startHoursAgo * 3600000;
        const endMs = now - endHoursAgo * 3600000;
        return points.filter(function(p) {
            var t = parseTimeMs(p.recorded_at);
            return t >= startMs && t <= endMs;
        });
    }

    function parseTimeMs(isoStr) {
        if (!isoStr) return 0;
        var s = isoStr.replace(' ', 'T');
        var d = new Date(s.endsWith('Z') ? s : s + 'Z');
        return isNaN(d) ? 0 : d.getTime();
    }

    // Speed color — 5-bucket gradient (Phase 3A)
    function speedColor5(speed) {
        if (speed < 5) return '#10b981';    // green (stopped/slow)
        if (speed < 15) return '#84cc16';   // lime
        if (speed < 35) return '#eab308';   // yellow
        if (speed < 55) return '#f97316';   // orange
        return '#ef4444';                    // red (fast)
    }

    // Bearing between two [lon,lat] points in degrees
    function bearing(p1, p2) {
        var dLon = (p2[0] - p1[0]) * Math.PI / 180;
        var lat1 = p1[1] * Math.PI / 180;
        var lat2 = p2[1] * Math.PI / 180;
        var y = Math.sin(dLon) * Math.cos(lat2);
        var x = Math.cos(lat1) * Math.sin(lat2) - Math.sin(lat1) * Math.cos(lat2) * Math.cos(dLon);
        return ((Math.atan2(y, x) * 180 / Math.PI) + 360) % 360;
    }

    // Haversine distance in miles
    function haversine(lat1, lon1, lat2, lon2) {
        var R = 3958.8;
        var dLat = (lat2 - lat1) * Math.PI / 180;
        var dLon = (lon2 - lon1) * Math.PI / 180;
        var a = Math.sin(dLat / 2) * Math.sin(dLat / 2) +
            Math.cos(lat1 * Math.PI / 180) * Math.cos(lat2 * Math.PI / 180) *
            Math.sin(dLon / 2) * Math.sin(dLon / 2);
        return R * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
    }

    // Stop detection: cluster consecutive points with speed < 2 mph within 0.001 deg
    function detectStops(points) {
        var stops = [];
        var i = 0;
        while (i < points.length) {
            if ((points[i].speed || 0) < 2) {
                var cluster = [points[i]];
                var j = i + 1;
                while (j < points.length &&
                       (points[j].speed || 0) < 2 &&
                       Math.abs(points[j].lat - points[i].lat) < 0.001 &&
                       Math.abs(points[j].lon - points[i].lon) < 0.001) {
                    cluster.push(points[j]);
                    j++;
                }
                if (cluster.length >= 2) {
                    var avgLat = cluster.reduce(function(s, p) { return s + p.lat; }, 0) / cluster.length;
                    var avgLon = cluster.reduce(function(s, p) { return s + p.lon; }, 0) / cluster.length;
                    stops.push({
                        lat: avgLat, lon: avgLon,
                        count: cluster.length,
                        startTime: cluster[0].recorded_at,
                        endTime: cluster[cluster.length - 1].recorded_at,
                    });
                }
                i = j;
            } else {
                i++;
            }
        }
        return stops;
    }

    // Build GeoJSON for all visible tracks → lines, dots, arrows
    function rebuildAllTrackSources() {
        var lineFeatures = [];
        var dotFeatures = [];
        var arrowFeatures = [];
        var multiTrack = countVisibleTracks() > 1;

        for (var entry of state.tracks) {
            var call = entry[0];
            var t = entry[1];
            if (!t.visible || !t.points || t.points.length < 2) continue;

            var coords = [];
            for (var i = 0; i < t.points.length; i++) {
                var p = t.points[i];
                if (typeof p.lon === 'number' && typeof p.lat === 'number' && isFinite(p.lon) && isFinite(p.lat)) {
                    coords.push({ coord: [p.lon, p.lat], point: p });
                }
            }
            if (coords.length < 2) continue;

            // Detect stops
            var stops = detectStops(t.points);

            // Per-segment LineString features
            for (var i = 1; i < coords.length; i++) {
                var speed = coords[i].point.speed || 0;
                var segColor = multiTrack ? t.color : speedColor5(speed);
                lineFeatures.push({
                    type: 'Feature',
                    geometry: { type: 'LineString', coordinates: [coords[i - 1].coord, coords[i].coord] },
                    properties: { color: segColor, speed: speed, callsign: call },
                });
            }

            // Dot features at each track point
            for (var i = 0; i < coords.length; i++) {
                var p = coords[i].point;
                var isStopped = (p.speed || 0) < 2;
                dotFeatures.push({
                    type: 'Feature',
                    geometry: { type: 'Point', coordinates: coords[i].coord },
                    properties: {
                        color: t.color,
                        callsign: call,
                        speed: p.speed || 0,
                        altitude: p.altitude || null,
                        time: p.recorded_at || '',
                        stopped: isStopped,
                    },
                });
            }

            // Arrow features at segment midpoints — adaptive decimation
            var arrowInterval = Math.max(1, Math.floor(coords.length / 30));
            for (var i = 1; i < coords.length; i += arrowInterval) {
                var mid = [
                    (coords[i - 1].coord[0] + coords[i].coord[0]) / 2,
                    (coords[i - 1].coord[1] + coords[i].coord[1]) / 2,
                ];
                var bear = bearing(coords[i - 1].coord, coords[i].coord);
                arrowFeatures.push({
                    type: 'Feature',
                    geometry: { type: 'Point', coordinates: mid },
                    properties: { color: t.color, bearing: bear, callsign: call },
                });
            }

            // Stop markers — larger dots
            for (var i = 0; i < stops.length; i++) {
                dotFeatures.push({
                    type: 'Feature',
                    geometry: { type: 'Point', coordinates: [stops[i].lon, stops[i].lat] },
                    properties: {
                        color: t.color,
                        callsign: call,
                        speed: 0,
                        altitude: null,
                        time: stops[i].startTime,
                        stopped: true,
                        stopMarker: true,
                        stopCount: stops[i].count,
                    },
                });
            }
        }

        var hasData = lineFeatures.length > 0;
        updateTracks(JSON.stringify({ type: 'FeatureCollection', features: lineFeatures }));
        updateTrackDots(JSON.stringify({ type: 'FeatureCollection', features: dotFeatures }));
        updateTrackArrows(JSON.stringify({ type: 'FeatureCollection', features: arrowFeatures }));
        setTracksVisible(hasData);
    }

    function countVisibleTracks() {
        var count = 0;
        for (var entry of state.tracks) {
            if (entry[1].visible) count++;
        }
        return count;
    }

    // ===== Track Panel =====

    function toggleTrackPanel(open) {
        state.trackPanelOpen = open !== undefined ? open : !state.trackPanelOpen;
        if (dom.trackPanel) {
            dom.trackPanel.style.display = state.trackPanelOpen ? 'flex' : 'none';
        }
    }

    function updateTrackButton() {
        if (dom.btnTracks) {
            var count = state.tracks.size;
            dom.btnTracks.textContent = count > 0 ? 'Tracks (' + count + ')' : 'Tracks';
            dom.btnTracks.classList.toggle('active', count > 0);
        }
    }

    function renderTrackPanel() {
        var panel = dom.trackPanel;
        if (!panel) return;

        var listEl = document.getElementById('track-list');
        if (!listEl) return;

        if (state.tracks.size === 0) {
            listEl.innerHTML = '<div class="track-empty">Click "Track" on a station to add tracks</div>';
            renderTrackStats(null);
            return;
        }

        var html = '';
        for (var entry of state.tracks) {
            var call = entry[0];
            var t = entry[1];
            var selected = state.selectedTrack === call ? ' selected' : '';
            var eyeIcon = t.visible ? '&#x1f441;' : '&#x1f441;&#xfe0e;';
            var eyeCls = t.visible ? 'track-eye' : 'track-eye track-eye-off';
            html += '<div class="track-item' + selected + '" data-call="' + esc(call) + '">';
            html += '<span class="track-swatch" style="background:' + t.color + '"></span>';
            html += '<span class="track-call">' + esc(call) + '</span>';
            html += '<span class="track-pts">' + (t.points ? t.points.length : 0) + ' pts</span>';
            html += '<button class="' + eyeCls + '" data-action="toggle" title="Toggle visibility">' + eyeIcon + '</button>';
            html += '<button class="track-remove" data-action="remove" title="Remove track">&times;</button>';
            html += '</div>';
        }
        listEl.innerHTML = html;

        // Click handlers
        listEl.querySelectorAll('.track-item').forEach(function(el) {
            el.addEventListener('click', function(e) {
                var action = e.target.dataset.action;
                var call = el.dataset.call;
                if (action === 'toggle') {
                    toggleTrackVisibility(call);
                } else if (action === 'remove') {
                    removeTrack(call);
                } else {
                    // Select for stats
                    state.selectedTrack = state.selectedTrack === call ? null : call;
                    renderTrackPanel();
                }
            });
        });

        // Render stats for selected track
        if (state.selectedTrack && state.tracks.has(state.selectedTrack)) {
            renderTrackStats(state.tracks.get(state.selectedTrack));
        } else {
            renderTrackStats(null);
        }
    }

    function renderTrackStats(trackEntry) {
        var el = document.getElementById('track-stats-panel');
        if (!el) return;

        if (!trackEntry || !trackEntry.points || trackEntry.points.length < 2) {
            el.innerHTML = '';
            return;
        }

        var pts = trackEntry.points;
        var totalDist = 0;
        var maxSpeed = 0;
        var speedSum = 0;
        var speedCount = 0;
        for (var i = 1; i < pts.length; i++) {
            totalDist += haversine(pts[i - 1].lat, pts[i - 1].lon, pts[i].lat, pts[i].lon);
            if (pts[i].speed != null) {
                maxSpeed = Math.max(maxSpeed, pts[i].speed);
                speedSum += pts[i].speed;
                speedCount++;
            }
        }
        var avgSpeed = speedCount > 0 ? speedSum / speedCount : 0;
        var stops = detectStops(pts);

        var startTime = parseTimeMs(pts[0].recorded_at);
        var endTime = parseTimeMs(pts[pts.length - 1].recorded_at);
        var durationMs = endTime - startTime;
        var durationStr = '';
        if (durationMs > 3600000) {
            durationStr = (durationMs / 3600000).toFixed(1) + 'h';
        } else {
            durationStr = Math.floor(durationMs / 60000) + 'm';
        }

        el.innerHTML = '<div class="track-stats-grid">' +
            '<div class="track-stat-mini"><span class="stat-val">' + pts.length + '</span><span class="stat-lbl">Points</span></div>' +
            '<div class="track-stat-mini"><span class="stat-val">' + totalDist.toFixed(1) + '</span><span class="stat-lbl">Miles</span></div>' +
            '<div class="track-stat-mini"><span class="stat-val">' + avgSpeed.toFixed(0) + '</span><span class="stat-lbl">Avg mph</span></div>' +
            '<div class="track-stat-mini"><span class="stat-val">' + maxSpeed.toFixed(0) + '</span><span class="stat-lbl">Max mph</span></div>' +
            '<div class="track-stat-mini"><span class="stat-val">' + durationStr + '</span><span class="stat-lbl">Duration</span></div>' +
            '<div class="track-stat-mini"><span class="stat-val">' + stops.length + '</span><span class="stat-lbl">Stops</span></div>' +
            '</div>';

        // Speed legend for single-track mode
        if (countVisibleTracks() <= 1) {
            el.innerHTML += '<div class="speed-legend">' +
                '<span class="speed-legend-title">Speed:</span>' +
                '<span class="speed-swatch" style="background:#10b981"></span><span class="speed-lbl">&lt;5</span>' +
                '<span class="speed-swatch" style="background:#84cc16"></span><span class="speed-lbl">5-15</span>' +
                '<span class="speed-swatch" style="background:#eab308"></span><span class="speed-lbl">15-35</span>' +
                '<span class="speed-swatch" style="background:#f97316"></span><span class="speed-lbl">35-55</span>' +
                '<span class="speed-swatch" style="background:#ef4444"></span><span class="speed-lbl">55+</span>' +
                '</div>';
        }
    }

    // ===== Time Slider =====

    var sliderDebounce = null;

    function onTimeSliderChange() {
        var startSlider = document.getElementById('track-time-start');
        var endSlider = document.getElementById('track-time-end');
        var startLabel = document.getElementById('track-time-start-label');
        var endLabel = document.getElementById('track-time-end-label');
        if (!startSlider || !endSlider) return;

        var startVal = parseInt(startSlider.value);
        var endVal = parseInt(endSlider.value);

        // Ensure start >= end (start is further in the past)
        if (startVal < endVal) {
            startSlider.value = endVal;
            startVal = endVal;
        }

        state.trackTimeStart = startVal;
        state.trackTimeEnd = endVal;

        if (startLabel) startLabel.textContent = startVal === 0 ? 'now' : '-' + startVal + 'h';
        if (endLabel) endLabel.textContent = endVal === 0 ? 'now' : '-' + endVal + 'h';

        // Debounced re-filter
        if (sliderDebounce) clearTimeout(sliderDebounce);
        sliderDebounce = setTimeout(function() {
            refilterAllTracks();
        }, 200);
    }

    function refilterAllTracks() {
        for (var entry of state.tracks) {
            var t = entry[1];
            if (t.cachedPoints) {
                t.points = filterTrackByTime(t.cachedPoints, state.trackTimeStart, state.trackTimeEnd);
            }
        }
        rebuildAllTrackSources();
        renderTrackPanel();
    }

    function setTimePreset(hours) {
        state.trackTimeStart = hours;
        state.trackTimeEnd = 0;
        var startSlider = document.getElementById('track-time-start');
        var endSlider = document.getElementById('track-time-end');
        if (startSlider) startSlider.value = hours;
        if (endSlider) endSlider.value = 0;
        onTimeSliderChange();
    }

    // ===== Rendering: Station List =====
    function renderStations() {
        const filter = state.searchFilter.toLowerCase();
        const sorted = Array.from(state.stations.values())
            .filter(s => matchesSourceFilter(s))
            .sort((a, b) => b.last_heard.localeCompare(a.last_heard));

        const filtered = filter
            ? sorted.filter(s =>
                s.callsign.toLowerCase().includes(filter) ||
                (s.comment && s.comment.toLowerCase().includes(filter)))
            : sorted;

        dom.stationCount.textContent = filtered.length;

        const rows = filtered.map(s => {
            const key = stationKey(s);
            const selected = key === state.selectedStation ? ' selected' : '';
            const typeCls = typeClass(s.station_type, s.weather);
            const callDisp = s.ssid > 0 ? `${s.callsign}-${s.ssid}` : s.callsign;
            const age = timeAgo(s.last_heard);
            const srcBadge = s.heard_via && s.heard_via.indexOf('tnc') >= 0
                ? '<span class="source-dot source-dot-rf" title="Heard via RF"></span>'
                : '';

            return `<tr class="station-row${selected}" data-key="${esc(key)}">
                <td class="station-call">${srcBadge}${esc(callDisp)}</td>
                <td><span class="type-badge ${typeCls}">${esc(classifyType(s.station_type, s.weather))}</span></td>
                <td class="station-time">${esc(age)}</td>
            </tr>`;
        });

        dom.stationTbody.innerHTML = rows.join('');

        dom.stationTbody.querySelectorAll('.station-row').forEach(tr => {
            tr.addEventListener('click', () => {
                selectStation(tr.dataset.key, { fly: true });
            });
        });
    }

    // ===== Rendering: Station Detail =====
    function renderStationDetail(station) {
        if (!station) {
            dom.stationDetail.innerHTML = '<div class="detail-empty">Select a station</div>';
            return;
        }

        const callDisp = station.ssid > 0 ? `${station.callsign}-${station.ssid}` : station.callsign;
        const typeName = classifyType(station.station_type, station.weather);
        const typeCls = typeClass(station.station_type, station.weather);
        const tracked = isTracked(callDisp);

        let html = '<div class="detail-content">';
        html += `<div class="detail-callsign">${esc(callDisp)}</div>`;
        html += `<div class="detail-type"><span class="type-badge ${typeCls}">${esc(typeName)}</span>`;
        if (station.heard_via) {
            html += ' ' + heardViaBadgesHtml(station.heard_via);
        }
        html += '</div>';

        if (station.lat != null && station.lon != null) {
            html += `<div><span class="label">Position:</span> ${station.lat.toFixed(4)}, ${station.lon.toFixed(4)}</div>`;
        }
        if (station.altitude != null) {
            html += `<div><span class="label">Altitude:</span> ${station.altitude.toFixed(0)} ft</div>`;
        }
        if (station.speed != null && station.speed > 0) {
            html += `<div><span class="label">Speed:</span> ${station.speed.toFixed(0)} mph`;
            if (station.course != null) html += ` @ ${station.course.toFixed(0)}&deg;`;
            html += '</div>';
        }
        if (station.comment) {
            html += `<div><span class="label">Comment:</span> ${esc(station.comment)}</div>`;
        }
        html += `<div><span class="label">Packets:</span> ${station.packet_count}</div>`;
        html += `<div><span class="label">Last heard:</span> ${esc(timeAgo(station.last_heard))}</div>`;

        if (station.weather) {
            html += renderWeather(station.weather);
        }

        html += '<div style="margin-top:8px">';
        html += '<button class="btn btn-sm btn-primary" id="btn-station-detail">Details</button>';
        if (station.lat != null) {
            var trackLabel = tracked ? 'Untrack' : 'Track';
            var trackCls = tracked ? 'btn-danger' : 'btn-secondary';
            html += ` <button class="btn btn-sm ${trackCls}" id="btn-track-station">${trackLabel}</button>`;
        }
        html += '</div>';

        html += '</div>';
        dom.stationDetail.innerHTML = html;

        const detailBtn = document.getElementById('btn-station-detail');
        if (detailBtn) {
            detailBtn.addEventListener('click', () => {
                if (typeof openStationModal === 'function') {
                    openStationModal(station);
                }
            });
        }

        const trackBtn = document.getElementById('btn-track-station');
        if (trackBtn) {
            trackBtn.addEventListener('click', () => {
                if (tracked) {
                    removeTrack(callDisp);
                } else {
                    addTrack(station.callsign, station.ssid, 48);
                }
                renderStationDetail(station);
            });
        }
    }

    function heardViaBadgesHtml(heardVia) {
        if (!heardVia) return '';
        let html = '';
        if (heardVia.indexOf('tnc') >= 0) html += '<span class="source-badge source-rf">RF</span> ';
        if (heardVia.indexOf('aprs-is') >= 0) html += '<span class="source-badge source-net">NET</span>';
        return html;
    }

    function renderWeather(wx) {
        let html = '<div class="detail-weather"><h4>Weather</h4>';
        if (wx.temperature != null) html += `<div>Temp: ${wx.temperature}&deg;F</div>`;
        if (wx.wind_speed != null) {
            html += `<div>Wind: ${wx.wind_speed} mph`;
            if (wx.wind_direction != null) html += ` @ ${wx.wind_direction}&deg;`;
            html += '</div>';
        }
        if (wx.wind_gust != null) html += `<div>Gust: ${wx.wind_gust} mph</div>`;
        if (wx.humidity != null) html += `<div>Humidity: ${wx.humidity}%</div>`;
        if (wx.barometric_pressure != null) html += `<div>Pressure: ${(wx.barometric_pressure / 10).toFixed(1)} hPa</div>`;
        if (wx.rain_last_hour != null) html += `<div>Rain (1h): ${wx.rain_last_hour / 100}" </div>`;
        if (wx.rain_24h != null) html += `<div>Rain (24h): ${wx.rain_24h / 100}"</div>`;
        if (wx.luminosity != null) html += `<div>Luminosity: ${wx.luminosity} W/m&sup2;</div>`;
        html += '</div>';
        return html;
    }

    // ===== Rendering: Packet Log =====
    function renderPackets() {
        const filtered = state.packets.filter(p => matchesPacketSourceFilter(p));
        dom.packetCount.textContent = `${filtered.length} packets`;

        const rows = filtered.slice(0, 200).map(p => {
            const typeCls = typeClass(p.packet_type);
            const time = formatTime(p.received_at);
            const srcBadge = p.source_type === 'tnc'
                ? '<span class="source-badge source-rf" style="font-size:10px">RF</span>'
                : p.source_type === 'aprs-is'
                ? '<span class="source-badge source-net" style="font-size:10px">NET</span>'
                : '';
            return `<tr class="packet-row" data-source="${esc(p.source)}">
                <td class="pkt-time">${esc(time)}</td>
                <td class="pkt-source">${esc(p.source)}${p.source_ssid > 0 ? '-' + p.source_ssid : ''}</td>
                <td class="pkt-dest">${esc(p.dest)}</td>
                <td><span class="type-badge ${typeCls}">${esc(p.packet_type || '?')}</span></td>
                <td>${srcBadge}</td>
                <td class="pkt-summary">${esc(p.summary || p.raw_info)}</td>
            </tr>`;
        });

        dom.packetTbody.innerHTML = rows.join('');

        dom.packetTbody.querySelectorAll('.packet-row').forEach(tr => {
            tr.addEventListener('click', () => {
                const src = tr.dataset.source;
                for (const [key, s] of state.stations) {
                    if (s.callsign === src) {
                        selectStation(key);
                        break;
                    }
                }
            });
        });
    }

    // ===== Station Selection =====
    function selectStation(key, opts) {
        state.selectedStation = key;
        const station = state.stations.get(key);
        renderStationDetail(station || null);
        renderStations();
        updateMapStations();

        if (opts && opts.fly && station && typeof station.lat === 'number' && typeof station.lon === 'number') {
            flyTo(station.lon, station.lat, 9);
        }
    }

    // ===== Map Integration =====
    function updateMapStations() {
        const geojson = stationsToGeoJSON();
        updateStations(JSON.stringify(geojson));
    }

    function stationsToGeoJSON() {
        const features = [];
        for (const [key, s] of state.stations) {
            if (typeof s.lat !== 'number' || typeof s.lon !== 'number') continue;
            if (!isFinite(s.lat) || !isFinite(s.lon)) continue;
            if (!matchesSourceFilter(s)) continue;

            const callDisp = s.ssid > 0 ? `${s.callsign}-${s.ssid}` : s.callsign;

            let ageMinutes = 0;
            if (s.last_heard) {
                const ts = s.last_heard.replace(' ', 'T');
                const then = new Date(ts.endsWith('Z') ? ts : ts + 'Z');
                if (!isNaN(then)) {
                    ageMinutes = (Date.now() - then.getTime()) / 60000;
                }
            }

            let wxLabel = '';
            if (s.weather) {
                const parts = [];
                if (s.weather.temperature != null) parts.push(s.weather.temperature + '\u00B0F');
                if (s.weather.wind_speed != null) parts.push(s.weather.wind_speed + 'mph');
                if (parts.length > 0) wxLabel = parts.join(' ');
            }

            features.push({
                type: 'Feature',
                geometry: {
                    type: 'Point',
                    coordinates: [s.lon, s.lat],
                },
                properties: {
                    callsign: callDisp,
                    stationType: classifyType(s.station_type, s.weather) || 'Unknown',
                    selected: key === state.selectedStation,
                    symbolTable: s.symbol_table || '',
                    symbolCode: s.symbol_code || '',
                    heardVia: s.heard_via || '',
                    ageMinutes: ageMinutes,
                    windDirection: s.weather ? (s.weather.wind_direction || 0) : 0,
                    hasWind: !!(s.weather && s.weather.wind_speed != null && s.weather.wind_speed > 0),
                    wxLabel: wxLabel,
                    hasMoved: s.has_moved || false,
                    lastPath: s.last_path || '',
                },
            });
        }
        return { type: 'FeatureCollection', features };
    }

    // Expose track state sync for station_detail.js "Show on Map" button
    // Wires legacy "Show on Map" into multi-track system
    window.syncTrackState = function(visible) {
        // If a station was shown on map via the old path, it's already in the
        // tracks source. Nothing else needed since multi-track manages visibility.
    };

    // Expose addStationTrack for station_detail.js
    window.addStationTrack = function(callsign, ssid, hours) {
        return addTrack(callsign, ssid, hours || 48);
    };

    // Expose removeStationTrack for station_detail.js
    window.removeStationTrack = function(call) {
        removeTrack(call);
    };

    // Expose isTracked for station_detail.js
    window.isStationTracked = function(callsign, ssid) {
        var call = ssid > 0 ? callsign + '-' + ssid : callsign;
        return isTracked(call);
    };

    // Expose track data for playback
    window.getTrackData = function() {
        var allPoints = [];
        for (var entry of state.tracks) {
            var t = entry[1];
            if (!t.visible || !t.points) continue;
            for (var i = 0; i < t.points.length; i++) {
                var p = t.points[i];
                if (typeof p.lon === 'number' && typeof p.lat === 'number' && isFinite(p.lon) && isFinite(p.lat)) {
                    allPoints.push({
                        lon: p.lon, lat: p.lat,
                        callsign: entry[0],
                        color: t.color,
                        speed: p.speed || 0,
                        altitude: p.altitude,
                        time: p.recorded_at,
                        timeMs: parseTimeMs(p.recorded_at),
                    });
                }
            }
        }
        allPoints.sort(function(a, b) { return a.timeMs - b.timeMs; });
        return allPoints;
    };

    // Map callbacks
    function onMapStationClick(callsign) {
        for (const [key, s] of state.stations) {
            const disp = s.ssid > 0 ? `${s.callsign}-${s.ssid}` : s.callsign;
            if (disp === callsign) {
                selectStation(key);
                if (typeof openStationModal === 'function') {
                    openStationModal(s);
                }
                return;
            }
        }
    }

    function onMapEmptyClick() {
        state.selectedStation = null;
        renderStationDetail(null);
        renderStations();
        updateMapStations();
    }

    // ===== Settings =====
    async function openSettings() {
        const config = await fetchConfig();
        if (!config) return;

        document.getElementById('cfg-tnc-enabled').checked = config.tnc.enabled;
        document.getElementById('cfg-tnc-host').value = config.tnc.host;
        document.getElementById('cfg-tnc-port').value = config.tnc.port;
        document.getElementById('cfg-aprs-enabled').checked = config.aprs_is.enabled;
        document.getElementById('cfg-aprs-host').value = config.aprs_is.host;
        document.getElementById('cfg-aprs-port').value = config.aprs_is.port;
        document.getElementById('cfg-aprs-callsign').value = config.aprs_is.callsign;
        document.getElementById('cfg-aprs-passcode').value = config.aprs_is.passcode;
        document.getElementById('cfg-aprs-filter').value = config.aprs_is.filter;
        document.getElementById('cfg-max-station-age').value = config.max_station_age_hours;
        document.getElementById('cfg-max-track-age').value = config.max_track_age_hours;

        dom.settingsStatus.style.display = 'none';
        dom.settingsModal.style.display = 'flex';
    }

    function closeSettings() {
        dom.settingsModal.style.display = 'none';
    }

    async function handleSaveSettings() {
        const current = await fetchConfig();
        if (!current) {
            dom.settingsStatus.className = 'settings-status settings-error';
            dom.settingsStatus.textContent = 'Error: could not load current config';
            dom.settingsStatus.style.display = 'block';
            return;
        }

        const config = Object.assign({}, current, {
            max_station_age_hours: parseInt(document.getElementById('cfg-max-station-age').value) || 48,
            max_track_age_hours: parseInt(document.getElementById('cfg-max-track-age').value) || 48,
            tnc: {
                enabled: document.getElementById('cfg-tnc-enabled').checked,
                host: document.getElementById('cfg-tnc-host').value,
                port: parseInt(document.getElementById('cfg-tnc-port').value) || 8001,
            },
            aprs_is: {
                enabled: document.getElementById('cfg-aprs-enabled').checked,
                host: document.getElementById('cfg-aprs-host').value,
                port: parseInt(document.getElementById('cfg-aprs-port').value) || 14580,
                callsign: document.getElementById('cfg-aprs-callsign').value,
                passcode: document.getElementById('cfg-aprs-passcode').value,
                filter: document.getElementById('cfg-aprs-filter').value,
            },
        });

        try {
            await saveConfig(config);
            closeSettings();
        } catch (e) {
            dom.settingsStatus.className = 'settings-status settings-error';
            dom.settingsStatus.textContent = 'Error: ' + e.message;
            dom.settingsStatus.style.display = 'block';
        }
    }

    // ===== Helpers =====
    function stationKey(s) {
        return s.ssid > 0 ? `${s.callsign}-${s.ssid}` : s.callsign;
    }

    function classifyType(stationType, weather) {
        if (weather) return 'Weather';
        switch (stationType) {
            case 'MicE': return 'Mobile';
            case 'Position': return 'Position';
            case 'Weather': return 'Weather';
            case 'Object': return 'Object';
            case 'Item': return 'Item';
            case 'Message': return 'Message';
            case 'Status': return 'Status';
            default: return stationType || 'Unknown';
        }
    }

    function typeClass(stationType, weather) {
        const t = classifyType(stationType, weather).toLowerCase();
        return 'type-' + t.replace(/[^a-z]/g, '');
    }

    function timeAgo(isoStr) {
        if (!isoStr) return '';
        const s = isoStr.replace(' ', 'T');
        const then = new Date(s.endsWith('Z') ? s : s + 'Z');
        if (isNaN(then)) return '';
        const now = new Date();
        const secs = Math.floor((now - then) / 1000);
        if (secs < 0) return 'just now';
        if (secs < 60) return secs + 's ago';
        if (secs < 3600) return Math.floor(secs / 60) + 'm ago';
        if (secs < 86400) return Math.floor(secs / 3600) + 'h ago';
        return Math.floor(secs / 86400) + 'd ago';
    }

    function formatTime(isoStr) {
        if (!isoStr) return '';
        const s = isoStr.replace(' ', 'T');
        const d = new Date(s.endsWith('Z') ? s : s + 'Z');
        if (isNaN(d)) return '';
        return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    }

    function esc(s) {
        if (s == null) return '';
        const d = document.createElement('div');
        d.textContent = String(s);
        return d.innerHTML;
    }

    // ===== Initialization =====
    function init() {
        initMap('map-container', -98.5, 39.8, 4, '', true);

        onStationClick(onMapStationClick);
        onMapClick(onMapEmptyClick);

        dom.searchInput.addEventListener('input', () => {
            state.searchFilter = dom.searchInput.value;
            renderStations();
        });

        document.querySelectorAll('.source-btn').forEach(btn => {
            btn.addEventListener('click', () => {
                document.querySelectorAll('.source-btn').forEach(b => b.classList.remove('active'));
                btn.classList.add('active');
                state.sourceFilter = btn.dataset.source;
                renderStations();
                renderPackets();
                updateMapStations();
            });
        });

        // Tracks button — toggle panel
        if (dom.btnTracks) {
            dom.btnTracks.addEventListener('click', () => {
                toggleTrackPanel();
            });
        }

        // Track panel close button
        var trackClose = document.getElementById('track-panel-close');
        if (trackClose) {
            trackClose.addEventListener('click', () => toggleTrackPanel(false));
        }

        // Time slider handlers
        var startSlider = document.getElementById('track-time-start');
        var endSlider = document.getElementById('track-time-end');
        if (startSlider) startSlider.addEventListener('input', onTimeSliderChange);
        if (endSlider) endSlider.addEventListener('input', onTimeSliderChange);

        // Time preset buttons
        document.querySelectorAll('.track-time-preset').forEach(function(btn) {
            btn.addEventListener('click', function() {
                document.querySelectorAll('.track-time-preset').forEach(function(b) { b.classList.remove('active'); });
                btn.classList.add('active');
                setTimePreset(parseInt(btn.dataset.hours));
            });
        });

        // Playback button in track panel
        var playBtn = document.getElementById('track-play-btn');
        if (playBtn) {
            playBtn.addEventListener('click', function() {
                if (typeof startPlayback === 'function') {
                    startPlayback();
                }
            });
        }

        // Track All / Clear All buttons
        var trackAllBtn = document.getElementById('track-all-btn');
        if (trackAllBtn) {
            trackAllBtn.addEventListener('click', function() { trackAllMoved(); });
        }
        var trackClearBtn = document.getElementById('track-clear-btn');
        if (trackClearBtn) {
            trackClearBtn.addEventListener('click', function() { clearAllTracks(); });
        }

        // Show FCC nearby hams toggle
        dom.showFccNearby.addEventListener('change', () => {
            if (typeof setFccNearbyVisible === 'function') {
                setFccNearbyVisible(dom.showFccNearby.checked);
            }
        });

        // Settings
        dom.btnSettings.addEventListener('click', openSettings);
        dom.settingsClose.addEventListener('click', closeSettings);
        dom.settingsCancel.addEventListener('click', closeSettings);
        dom.settingsSave.addEventListener('click', handleSaveSettings);
        dom.settingsModal.addEventListener('click', (e) => {
            if (e.target === dom.settingsModal) closeSettings();
        });

        // Station detail modal
        if (typeof stationDetailInit === 'function') {
            stationDetailInit();
        }

        connectWebSocket();

        setInterval(() => {
            renderStations();
            updateMapStations();
        }, 30000);
    }

    // Expose station position lookup for path visualization
    window.getStationPosition = function(callsign) {
        for (var entry of state.stations) {
            var s = entry[1];
            var disp = s.ssid > 0 ? s.callsign + '-' + s.ssid : s.callsign;
            if (disp === callsign && typeof s.lat === 'number' && typeof s.lon === 'number') {
                return [s.lon, s.lat];
            }
        }
        return null;
    };

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();

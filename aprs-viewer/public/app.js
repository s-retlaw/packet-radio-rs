// APRS Viewer — Main Application
// Vanilla JS frontend: WebSocket client, state management, DOM rendering

(function() {
    'use strict';

    // ===== State =====
    const state = {
        stations: new Map(),    // callsign -> StationRow
        packets: [],            // PacketRow[], newest first
        selectedStation: null,  // callsign string or null
        connected: false,
        searchFilter: '',
        sourceFilter: 'all',   // "all", "tnc", or "aprs-is"
        showTracks: false,
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
        showTracks: document.getElementById('show-tracks'),
        statusDot: document.getElementById('status-dot'),
        statusText: document.getElementById('status-text'),
        btnSettings: document.getElementById('btn-settings'),
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
                // Load initial packets
                state.packets = event.packets || [];
                renderPackets();
                // Fetch stations via REST
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

        // If this is the selected station, refresh detail
        if (state.selectedStation === key) {
            renderStationDetail(station);
        }

        // Trigger pulse animation on map
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

    async function fetchStationTrack(callsign, ssid) {
        try {
            const call = ssid > 0 ? `${callsign}-${ssid}` : callsign;
            const resp = await fetch(`/api/stations/${encodeURIComponent(call)}/track?hours=24`);
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

        // Build HTML
        const rows = filtered.map(s => {
            const key = stationKey(s);
            const selected = key === state.selectedStation ? ' selected' : '';
            const typeCls = typeClass(s.station_type, s.weather);
            const callDisp = s.ssid > 0 ? `${s.callsign}-${s.ssid}` : s.callsign;
            const age = timeAgo(s.last_heard);
            // Source indicator
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

        // Click handlers — fly to station on sidebar click
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

        let html = '<div class="detail-content">';
        html += `<div class="detail-callsign">${esc(callDisp)}</div>`;
        html += `<div class="detail-type"><span class="type-badge ${typeCls}">${esc(typeName)}</span>`;
        // Source badges
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

        // Weather section
        if (station.weather) {
            html += renderWeather(station.weather);
        }

        // Detail button
        html += `<div style="margin-top:8px"><button class="btn btn-sm btn-primary" id="btn-station-detail">Details</button>`;
        if (station.lat != null) {
            html += ` <button class="btn btn-sm btn-secondary" id="btn-show-track">Show Track</button>`;
        }
        html += '</div>';

        html += '</div>';
        dom.stationDetail.innerHTML = html;

        // Detail button handler — open modal
        const detailBtn = document.getElementById('btn-station-detail');
        if (detailBtn) {
            detailBtn.addEventListener('click', () => {
                if (typeof openStationModal === 'function') {
                    openStationModal(station);
                }
            });
        }

        // Track button handler
        const trackBtn = document.getElementById('btn-show-track');
        if (trackBtn) {
            trackBtn.addEventListener('click', () => loadTrack(station));
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

        // Click to select station from packet
        dom.packetTbody.querySelectorAll('.packet-row').forEach(tr => {
            tr.addEventListener('click', () => {
                const src = tr.dataset.source;
                // Find matching station
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
    // opts.fly: if true, center+zoom the map on the station
    function selectStation(key, opts) {
        state.selectedStation = key;
        const station = state.stations.get(key);
        renderStationDetail(station || null);
        renderStations(); // re-render to update selection highlight
        updateMapStations();

        // Center map only when explicitly requested (sidebar click)
        if (opts && opts.fly && station && typeof station.lat === 'number' && typeof station.lon === 'number') {
            flyTo(station.lon, station.lat, 9);
        }

        // Clear any stale track from previous station
        clearTracks();
        state.showTracks = false;
        dom.showTracks.checked = false;
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

            // Calculate age in minutes for opacity
            let ageMinutes = 0;
            if (s.last_heard) {
                const ts = s.last_heard.replace(' ', 'T');
                const then = new Date(ts.endsWith('Z') ? ts : ts + 'Z');
                if (!isNaN(then)) {
                    ageMinutes = (Date.now() - then.getTime()) / 60000;
                }
            }

            // Build weather summary for label
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

    async function loadTrack(station) {
        const track = await fetchStationTrack(station.callsign, station.ssid);
        if (track.length < 2) return;

        const coordinates = track
            .filter(t => typeof t.lon === 'number' && typeof t.lat === 'number' && isFinite(t.lon) && isFinite(t.lat))
            .map(t => [t.lon, t.lat]);
        const geojson = {
            type: 'FeatureCollection',
            features: [{
                type: 'Feature',
                geometry: { type: 'LineString', coordinates },
                properties: {
                    callsign: station.callsign,
                    points: track.length,
                },
            }],
        };

        updateTracks(JSON.stringify(geojson));
        setTracksVisible(true);
        state.showTracks = true;
        dom.showTracks.checked = true;
    }

    // Expose track state sync for station_detail.js "Show on Map" button
    window.syncTrackState = function(visible) {
        state.showTracks = visible;
        dom.showTracks.checked = visible;
    };

    // Map callbacks
    function onMapStationClick(callsign) {
        // Find station by callsign display name
        for (const [key, s] of state.stations) {
            const disp = s.ssid > 0 ? `${s.callsign}-${s.ssid}` : s.callsign;
            if (disp === callsign) {
                selectStation(key);
                // Also open modal
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
        // Fetch current config to preserve non-editable fields
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
        // Initialize map
        initMap('map-container', -98.5, 39.8, 4, '', true);

        // Register map callbacks
        onStationClick(onMapStationClick);
        onMapClick(onMapEmptyClick);

        // Search filter
        dom.searchInput.addEventListener('input', () => {
            state.searchFilter = dom.searchInput.value;
            renderStations();
        });

        // Source filter buttons
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

        // Show tracks toggle
        dom.showTracks.addEventListener('change', () => {
            state.showTracks = dom.showTracks.checked;
            setTracksVisible(state.showTracks);
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

        // Start WebSocket
        connectWebSocket();

        // Refresh timeAgo displays periodically
        setInterval(() => {
            renderStations();
            updateMapStations(); // refresh age-based opacity
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

    // Start when DOM is ready
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();

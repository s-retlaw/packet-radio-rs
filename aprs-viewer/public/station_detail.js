// Station Detail Modal — 4-tab detail view with weather charts, tracks, packets
(function() {
    'use strict';

    var currentStation = null;
    var weatherChart = null;
    var windChart = null;
    var pressureChart = null;
    var humidityChart = null;
    var rainChart = null;
    var windDirChart = null;
    var altitudeChart = null;
    var currentWeatherHours = 6;
    var currentTrackHours = 24;

    function esc(s) {
        if (s == null) return '';
        var d = document.createElement('div');
        d.textContent = String(s);
        return d.innerHTML;
    }

    function timeAgo(isoStr) {
        if (!isoStr) return '';
        var s = isoStr.replace(' ', 'T');
        var then = new Date(s.endsWith('Z') ? s : s + 'Z');
        if (isNaN(then)) return '';
        var now = new Date();
        var secs = Math.floor((now - then) / 1000);
        if (secs < 0) return 'just now';
        if (secs < 60) return secs + 's ago';
        if (secs < 3600) return Math.floor(secs / 60) + 'm ago';
        if (secs < 86400) return Math.floor(secs / 3600) + 'h ago';
        return Math.floor(secs / 86400) + 'd ago';
    }

    function formatTime(isoStr) {
        if (!isoStr) return '';
        var s = isoStr.replace(' ', 'T');
        var d = new Date(s.endsWith('Z') ? s : s + 'Z');
        if (isNaN(d)) return '';
        return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
    }

    // Source badge HTML
    function sourceBadge(sourceType) {
        if (sourceType === 'tnc') {
            return '<span class="source-badge source-rf">RF</span>';
        } else if (sourceType === 'aprs-is') {
            return '<span class="source-badge source-net">NET</span>';
        }
        return '<span class="source-badge source-unknown">?</span>';
    }

    function heardViaBadges(heardVia) {
        if (!heardVia) return '';
        var parts = heardVia.split(',');
        var html = '';
        if (parts.indexOf('tnc') >= 0) {
            html += '<span class="source-badge source-rf">RF</span> ';
        }
        if (parts.indexOf('aprs-is') >= 0) {
            html += '<span class="source-badge source-net">NET</span>';
        }
        return html;
    }

    // Render the digipeater path as a visual chain
    function renderPath(pathStr) {
        if (!pathStr) return '<span class="text-muted">Direct</span>';
        var parts = pathStr.split(',');
        var html = '<div class="path-chain">';
        for (var i = 0; i < parts.length; i++) {
            if (i > 0) html += '<span class="path-arrow">&rarr;</span>';
            var p = parts[i].trim();
            var used = p.endsWith('*');
            var name = used ? p.slice(0, -1) : p;
            var cls = used ? 'path-node path-node--used' : 'path-node';
            html += '<span class="' + cls + '">' + esc(name) + '</span>';
        }
        html += '</div>';
        return html;
    }

    // Open station detail modal
    window.openStationModal = function(station) {
        currentStation = station;
        var modal = document.getElementById('station-modal');
        if (!modal) return;

        // Show callsign in modal header
        var callEl = document.getElementById('station-modal-call');
        if (callEl) {
            var callDisp = station.ssid > 0 ? station.callsign + '-' + station.ssid : station.callsign;
            callEl.textContent = callDisp;
        }

        modal.style.display = 'flex';
        switchTab('info');
        renderInfoTab(station);
    };

    window.closeStationModal = function() {
        var modal = document.getElementById('station-modal');
        if (modal) modal.style.display = 'none';
        destroyCharts();
        currentStation = null;
    };

    function switchTab(tabName) {
        var tabs = document.querySelectorAll('.modal-tab');
        var panels = document.querySelectorAll('.tab-panel');
        tabs.forEach(function(t) { t.classList.toggle('active', t.dataset.tab === tabName); });
        panels.forEach(function(p) { p.classList.toggle('active', p.id === 'tab-' + tabName); });

        if (tabName === 'weather' && currentStation) loadWeatherTab(currentStation);
        if (tabName === 'track' && currentStation) loadTrackTab(currentStation);
        if (tabName === 'packets' && currentStation) loadPacketsTab(currentStation);
    }

    // === Info Tab ===
    function renderInfoTab(s) {
        var panel = document.getElementById('tab-info');
        if (!panel) return;

        var callDisp = s.ssid > 0 ? s.callsign + '-' + s.ssid : s.callsign;
        var symDesc = getSymbolDescription(s.symbol_table || '', s.symbol_code || '');

        var html = '<div class="detail-header">';
        html += '<div class="detail-callsign">' + esc(callDisp) + '</div>';
        html += '<div class="detail-meta">';
        html += '<span class="type-badge type-' + (s.station_type || 'unknown').toLowerCase().replace(/[^a-z]/g, '') + '">' + esc(s.station_type) + '</span>';
        html += ' <span class="text-muted">' + esc(symDesc) + '</span>';
        html += '</div>';
        html += '<div class="detail-source">' + heardViaBadges(s.heard_via) + '</div>';
        html += '</div>';

        html += '<div class="detail-fields">';
        if (s.lat != null && s.lon != null) {
            html += '<div class="detail-field"><span class="label">Position:</span> <span class="copy-text" title="Click to copy">' + s.lat.toFixed(5) + ', ' + s.lon.toFixed(5) + '</span></div>';
        }
        if (s.altitude != null) {
            html += '<div class="detail-field"><span class="label">Altitude:</span> ' + s.altitude.toFixed(0) + ' ft</div>';
        }
        if (s.speed != null && s.speed > 0) {
            html += '<div class="detail-field"><span class="label">Speed:</span> ' + s.speed.toFixed(0) + ' mph';
            if (s.course != null) html += ' @ ' + s.course.toFixed(0) + '&deg;';
            html += '</div>';
        }
        if (s.comment) {
            html += '<div class="detail-field"><span class="label">Comment:</span> ' + esc(s.comment) + '</div>';
        }
        html += '<div class="detail-field"><span class="label">Packets:</span> ' + s.packet_count + '</div>';
        html += '<div class="detail-field"><span class="label">Last heard:</span> ' + esc(timeAgo(s.last_heard)) + '</div>';
        html += '</div>';

        // FCC info placeholder
        html += '<div id="detail-fcc"></div>';

        // Path display (fetch most recent packet)
        html += '<div class="detail-path" id="detail-path"><span class="label">Path:</span> <span class="text-muted">Loading...</span></div>';

        // Current weather
        if (s.weather) {
            html += renderWeatherCards(s.weather);
        }

        panel.innerHTML = html;

        // Copy coordinates on click
        panel.querySelectorAll('.copy-text').forEach(function(el) {
            el.addEventListener('click', function() {
                navigator.clipboard.writeText(el.textContent).catch(function() {});
                el.classList.add('copied');
                setTimeout(function() { el.classList.remove('copied'); }, 1000);
            });
        });

        // Fetch FCC licensee info
        fetchFccInfo(s);

        // Fetch path from most recent packet
        fetchStationPath(s);

        // Show/hide tabs based on data
        var wxTab = document.querySelector('[data-tab="weather"]');
        var trackTab = document.querySelector('[data-tab="track"]');
        if (wxTab) wxTab.style.display = s.weather ? '' : 'none';
        if (trackTab) trackTab.style.display = s.has_moved ? '' : 'none';
    }

    function renderWeatherCards(wx) {
        var html = '<div class="wx-cards">';
        if (wx.temperature != null) {
            html += '<div class="wx-card"><div class="wx-value">' + wx.temperature + '&deg;F</div><div class="wx-label">Temperature</div></div>';
        }
        if (wx.wind_speed != null) {
            var windInfo = wx.wind_speed + ' mph';
            if (wx.wind_direction != null) windInfo += ' @ ' + wx.wind_direction + '&deg;';
            html += '<div class="wx-card"><div class="wx-value">' + windInfo + '</div><div class="wx-label">Wind</div></div>';
        }
        if (wx.wind_gust != null) {
            html += '<div class="wx-card"><div class="wx-value">' + wx.wind_gust + ' mph</div><div class="wx-label">Gust</div></div>';
        }
        if (wx.humidity != null) {
            html += '<div class="wx-card"><div class="wx-value">' + wx.humidity + '%</div><div class="wx-label">Humidity</div></div>';
        }
        if (wx.barometric_pressure != null) {
            html += '<div class="wx-card"><div class="wx-value">' + (wx.barometric_pressure / 10).toFixed(1) + '</div><div class="wx-label">Pressure (hPa)</div></div>';
        }
        if (wx.rain_last_hour != null) {
            html += '<div class="wx-card"><div class="wx-value">' + (wx.rain_last_hour / 100).toFixed(2) + '"</div><div class="wx-label">Rain (1h)</div></div>';
        }
        if (wx.luminosity != null) {
            html += '<div class="wx-card"><div class="wx-value">' + wx.luminosity + '</div><div class="wx-label">Luminosity (W/m&sup2;)</div></div>';
        }
        html += '</div>';
        return html;
    }

    async function fetchStationPath(station) {
        var call = station.ssid > 0 ? station.callsign + '-' + station.ssid : station.callsign;
        try {
            var resp = await fetch('/api/stations/' + encodeURIComponent(call) + '/packets?limit=1');
            if (!resp.ok) return;
            var packets = await resp.json();
            var pathEl = document.getElementById('detail-path');
            if (pathEl && packets.length > 0) {
                pathEl.innerHTML = '<span class="label">Path:</span> ' + renderPath(packets[0].path);
            } else if (pathEl) {
                pathEl.innerHTML = '<span class="label">Path:</span> <span class="text-muted">No path data</span>';
            }
        } catch(e) { /* ignore */ }
    }

    // === FCC Lookup ===
    async function fetchFccInfo(station) {
        var el = document.getElementById('detail-fcc');
        if (!el) return;

        var call = station.ssid > 0 ? station.callsign + '-' + station.ssid : station.callsign;
        try {
            var resp = await fetch('/api/stations/' + encodeURIComponent(call) + '/fcc');
            if (resp.status === 404 || resp.status === 503) {
                // No FCC record or DB not loaded — show nothing
                return;
            }
            if (!resp.ok) return;
            var fcc = await resp.json();

            var classBadge = '';
            var cls = fcc.operator_class || '';
            if (cls === 'Extra') classBadge = 'fcc-class-extra';
            else if (cls === 'General') classBadge = 'fcc-class-general';
            else if (cls === 'Technician') classBadge = 'fcc-class-tech';
            else if (cls === 'Advanced') classBadge = 'fcc-class-advanced';
            else if (cls === 'Novice') classBadge = 'fcc-class-novice';

            var html = '<div class="fcc-info">';
            html += '<div class="fcc-name">' + esc(fcc.name) + '</div>';
            html += '<div class="fcc-details">';
            if (cls) {
                html += '<span class="fcc-class-badge ' + classBadge + '">' + esc(cls) + '</span> ';
            }
            var location = [fcc.city, fcc.state].filter(function(x) { return x; }).join(', ');
            if (fcc.zip_code) location += ' ' + fcc.zip_code;
            if (location.trim()) {
                html += '<span class="fcc-location">' + esc(location.trim()) + '</span>';
            }
            html += '</div>';
            if (fcc.grant_date || fcc.expired_date) {
                html += '<div class="fcc-dates text-muted">';
                html += 'Licensed: ' + esc(fcc.grant_date || '?') + ' \u2014 ' + esc(fcc.expired_date || '?');
                html += '</div>';
            }
            if (fcc.previous_call_sign) {
                html += '<div class="fcc-prev text-muted">Previous: <a href="#" class="fcc-prev-link" data-call="' + esc(fcc.previous_call_sign) + '">' + esc(fcc.previous_call_sign) + '</a></div>';
            }
            html += '</div>';

            el.innerHTML = html;

            // Make previous callsign clickable
            el.querySelectorAll('.fcc-prev-link').forEach(function(link) {
                link.addEventListener('click', function(e) {
                    e.preventDefault();
                    lookupFccCall(link.dataset.call);
                });
            });
        } catch(e) { /* ignore */ }
    }

    // Quick FCC lookup for a clicked previous callsign — show in a mini popup
    async function lookupFccCall(call) {
        try {
            var resp = await fetch('/api/stations/' + encodeURIComponent(call) + '/fcc');
            if (!resp.ok) return;
            var fcc = await resp.json();
            var msg = fcc.callsign + ': ' + fcc.name;
            if (fcc.operator_class) msg += ' (' + fcc.operator_class + ')';
            if (fcc.city) msg += ' \u2014 ' + fcc.city + ', ' + fcc.state;
            alert(msg);
        } catch(e) { /* ignore */ }
    }

    // === Weather Tab ===
    var cachedWeatherData = null; // fetched once at 48h, reused for all ranges

    async function loadWeatherTab(station) {
        var panel = document.getElementById('tab-weather');
        if (!panel) return;

        panel.innerHTML = '<div class="text-muted" style="padding:12px">Loading weather history...</div>';

        // Fetch full 48h of data once
        var call = station.ssid > 0 ? station.callsign + '-' + station.ssid : station.callsign;
        try {
            var resp = await fetch('/api/stations/' + encodeURIComponent(call) + '/weather-history?hours=48');
            if (!resp.ok) return;
            cachedWeatherData = await resp.json();
        } catch(e) {
            console.error('Weather history fetch error:', e);
            panel.innerHTML = '<div class="text-muted" style="padding:12px">Failed to load weather data</div>';
            return;
        }

        if (!cachedWeatherData || cachedWeatherData.length === 0) {
            panel.innerHTML = '<div class="text-muted" style="padding:12px">No weather history data</div>';
            return;
        }

        // Determine data age to filter applicable time ranges
        var now = Date.now();
        var oldestTime = parseTime(cachedWeatherData[0].recorded_at);
        var dataAgeHours = oldestTime ? (now - oldestTime.getTime()) / 3600000 : 0;

        // Show ranges up to the first one that covers all data, hide the rest.
        // e.g., data is 6.5h old → show 3h (zoom), 6h, 12h (covers all); skip 24h, 48h
        var allRanges = [3, 6, 12, 24, 48];
        var coverRange = allRanges.find(function(h) { return h >= dataAgeHours; }) || 48;
        var applicableRanges = allRanges.filter(function(h) { return h <= coverRange; });
        if (applicableRanges.length === 0) applicableRanges = [3];

        // Pick default: the smallest range that covers all data, or the largest applicable
        if (applicableRanges.indexOf(currentWeatherHours) < 0) {
            currentWeatherHours = coverRange || applicableRanges[applicableRanges.length - 1];
        }

        var btnsHtml = '<div class="time-range-btns" id="wx-range-btns">';
        for (var h = 0; h < applicableRanges.length; h++) {
            var active = applicableRanges[h] === currentWeatherHours ? ' active' : '';
            btnsHtml += '<button class="range-btn' + active + '" data-hours="' + applicableRanges[h] + '">' + applicableRanges[h] + 'h</button>';
        }
        btnsHtml += '</div>';
        panel.innerHTML = btnsHtml + '<div class="chart-grid" id="wx-charts"></div>';

        // Range button handlers — just re-render from cache, no re-fetch
        panel.querySelectorAll('.range-btn').forEach(function(btn) {
            btn.addEventListener('click', function() {
                panel.querySelectorAll('.range-btn').forEach(function(b) { b.classList.remove('active'); });
                btn.classList.add('active');
                currentWeatherHours = parseInt(btn.dataset.hours);
                renderWeatherCharts(cachedWeatherData, currentWeatherHours);
            });
        });

        renderWeatherCharts(cachedWeatherData, currentWeatherHours);
    }

    function destroyCharts() {
        [weatherChart, windChart, windDirChart, pressureChart, humidityChart, rainChart, altitudeChart].forEach(function(c) {
            if (c) c.destroy();
        });
        weatherChart = windChart = windDirChart = pressureChart = humidityChart = rainChart = altitudeChart = null;
    }

    function chartDefaults(hours, data) {
        var now = new Date();
        var minTime = new Date(now.getTime() - (hours || 6) * 3600000);

        // If we have data older than the time window, expand min to include it
        if (data && data.length > 0) {
            var oldest = parseTime(data[0].recorded_at);
            if (oldest && oldest < minTime) {
                minTime = new Date(oldest.getTime() - 5 * 60000); // 5 min padding
            }
        }

        return {
            responsive: true,
            maintainAspectRatio: false,
            animation: { duration: 300 },
            scales: {
                x: {
                    type: 'time',
                    min: minTime,
                    max: now,
                    time: { tooltipFormat: 'HH:mm' },
                    grid: { color: 'rgba(255,255,255,0.06)' },
                    ticks: { color: '#a1a1aa', maxTicksLimit: 8 },
                },
                y: {
                    grid: { color: 'rgba(255,255,255,0.06)' },
                    ticks: { color: '#a1a1aa' },
                }
            },
            plugins: {
                legend: { display: false },
            }
        };
    }

    function parseTime(s) {
        if (!s) return null;
        var ts = s.replace(' ', 'T');
        return new Date(ts.endsWith('Z') ? ts : ts + 'Z');
    }

    // Convert raw data array + field name into {x, y} points for Chart.js time scale.
    // Includes ALL data — the chart min/max controls the visible viewport.
    function toXY(data, field, transform) {
        var points = [];
        for (var i = 0; i < data.length; i++) {
            var t = parseTime(data[i].recorded_at);
            if (!t) continue;
            var v = data[i][field];
            if (v == null) continue;
            if (transform) v = transform(v);
            points.push({ x: t, y: v });
        }
        return points;
    }

    function renderWeatherCharts(data, hours) {
        destroyCharts();
        var grid = document.getElementById('wx-charts');
        if (!grid) return;
        grid.innerHTML = '';

        if (!data || data.length === 0) {
            grid.innerHTML = '<div class="text-muted" style="padding:12px;grid-column:1/-1">No weather history data</div>';
            return;
        }
        if (typeof Chart === 'undefined') return;

        var defaults = chartDefaults(hours, data);

        // Build list of charts that have data in the requested window
        var charts = [];

        var tempPts = toXY(data, 'temperature');
        if (tempPts.length > 0) {
            charts.push({ id: 'chart-temp', build: function(ctx) {
                weatherChart = new Chart(ctx, {
                    type: 'line',
                    data: { datasets: [{
                        label: 'Temperature', data: tempPts,
                        borderColor: '#f59e0b', backgroundColor: 'rgba(245,158,11,0.1)',
                        fill: true, tension: 0.3, pointRadius: 1,
                    }] },
                    options: Object.assign({}, defaults, {
                        plugins: { legend: { display: false }, title: { display: true, text: 'Temperature (\u00B0F)', color: '#a1a1aa' } }
                    })
                });
            }});
        }

        var windPts = toXY(data, 'wind_speed');
        var gustPts = toXY(data, 'wind_gust');
        if (windPts.length > 0 || gustPts.length > 0) {
            charts.push({ id: 'chart-wind', build: function(ctx) {
                windChart = new Chart(ctx, {
                    type: 'line',
                    data: { datasets: [
                        { label: 'Wind', data: windPts, borderColor: '#3b82f6', backgroundColor: 'rgba(59,130,246,0.1)', fill: true, tension: 0.3, pointRadius: 1 },
                        { label: 'Gust', data: gustPts, borderColor: '#93c5fd', backgroundColor: 'rgba(147,197,253,0.05)', fill: true, tension: 0.3, pointRadius: 1, borderDash: [4, 4] }
                    ] },
                    options: Object.assign({}, defaults, {
                        plugins: { legend: { display: true, labels: { color: '#a1a1aa' } }, title: { display: true, text: 'Wind (mph)', color: '#a1a1aa' } }
                    })
                });
            }});
        }

        var dirPts = toXY(data, 'wind_direction');
        if (dirPts.length > 0) {
            charts.push({ id: 'chart-wind-dir', build: function(ctx) {
                windDirChart = new Chart(ctx, {
                    type: 'line',
                    data: { datasets: [{
                        label: 'Direction', data: dirPts,
                        borderColor: '#60a5fa', backgroundColor: 'rgba(96,165,250,0.1)',
                        fill: false, tension: 0, pointRadius: 2, borderWidth: 1,
                    }] },
                    options: Object.assign({}, defaults, {
                        scales: { x: defaults.scales.x, y: Object.assign({}, defaults.scales.y, { min: 0, max: 360, ticks: Object.assign({}, defaults.scales.y.ticks, { stepSize: 90 }) }) },
                        plugins: { legend: { display: false }, title: { display: true, text: 'Wind Direction (\u00B0)', color: '#a1a1aa' } }
                    })
                });
            }});
        }

        var pressPts = toXY(data, 'barometric_pressure', function(v) { return v / 10; });
        if (pressPts.length > 0) {
            charts.push({ id: 'chart-pressure', build: function(ctx) {
                pressureChart = new Chart(ctx, {
                    type: 'line',
                    data: { datasets: [{
                        label: 'Pressure', data: pressPts,
                        borderColor: '#a855f7', backgroundColor: 'rgba(168,85,247,0.1)',
                        fill: true, tension: 0.3, pointRadius: 1,
                    }] },
                    options: Object.assign({}, defaults, {
                        plugins: { legend: { display: false }, title: { display: true, text: 'Pressure (hPa)', color: '#a1a1aa' } }
                    })
                });
            }});
        }

        var humidPts = toXY(data, 'humidity');
        if (humidPts.length > 0) {
            charts.push({ id: 'chart-humidity', build: function(ctx) {
                humidityChart = new Chart(ctx, {
                    type: 'line',
                    data: { datasets: [{
                        label: 'Humidity', data: humidPts,
                        borderColor: '#14b8a6', backgroundColor: 'rgba(20,184,166,0.1)',
                        fill: true, tension: 0.3, pointRadius: 1,
                    }] },
                    options: Object.assign({}, defaults, {
                        scales: { x: defaults.scales.x, y: Object.assign({}, defaults.scales.y, { min: 0, max: 100 }) },
                        plugins: { legend: { display: false }, title: { display: true, text: 'Humidity (%)', color: '#a1a1aa' } }
                    })
                });
            }});
        }

        var rainPts = toXY(data, 'rain_last_hour', function(v) { return v / 100; });
        rainPts = rainPts.filter(function(p) { return p.y > 0; });
        if (rainPts.length > 0) {
            charts.push({ id: 'chart-rain', build: function(ctx) {
                rainChart = new Chart(ctx, {
                    type: 'bar',
                    data: { datasets: [{
                        label: 'Rain', data: rainPts,
                        backgroundColor: 'rgba(59,130,246,0.5)', borderColor: '#3b82f6', borderWidth: 1,
                    }] },
                    options: Object.assign({}, defaults, {
                        plugins: { legend: { display: false }, title: { display: true, text: 'Rainfall (inches/hr)', color: '#a1a1aa' } }
                    })
                });
            }});
        }

        if (charts.length === 0) {
            grid.innerHTML = '<div class="text-muted" style="padding:12px;grid-column:1/-1">No chartable weather data in this time range</div>';
            return;
        }

        // Create only the containers that have data
        for (var i = 0; i < charts.length; i++) {
            var div = document.createElement('div');
            div.className = 'chart-container';
            var canvas = document.createElement('canvas');
            canvas.id = charts[i].id;
            div.appendChild(canvas);
            grid.appendChild(div);
        }

        // Build charts after containers are in DOM
        for (var j = 0; j < charts.length; j++) {
            var ctx = document.getElementById(charts[j].id);
            if (ctx) charts[j].build(ctx.getContext('2d'));
        }
    }

    // === Track Tab ===
    async function loadTrackTab(station) {
        var panel = document.getElementById('tab-track');
        if (!panel) return;

        panel.innerHTML = '<div class="time-range-btns" id="track-range-btns">' +
            '<button class="range-btn" data-hours="3">3h</button>' +
            '<button class="range-btn" data-hours="6">6h</button>' +
            '<button class="range-btn" data-hours="12">12h</button>' +
            '<button class="range-btn active" data-hours="24">24h</button>' +
            '<button class="range-btn" data-hours="48">48h</button>' +
            '</div>' +
            '<div id="track-stats"></div>' +
            '<div style="margin-top:8px"><button class="btn btn-primary btn-sm" id="btn-track-map">Show on Map</button></div>' +
            '<div id="altitude-container"></div>';

        panel.querySelectorAll('.range-btn').forEach(function(btn) {
            btn.addEventListener('click', function() {
                panel.querySelectorAll('.range-btn').forEach(function(b) { b.classList.remove('active'); });
                btn.classList.add('active');
                currentTrackHours = parseInt(btn.dataset.hours);
                fetchAndRenderTrack(station);
            });
        });

        document.getElementById('btn-track-map').addEventListener('click', function() {
            fetchAndShowTrackOnMap(station);
        });

        fetchAndRenderTrack(station);
    }

    async function fetchTrack(station, hours) {
        var call = station.ssid > 0 ? station.callsign + '-' + station.ssid : station.callsign;
        try {
            var resp = await fetch('/api/stations/' + encodeURIComponent(call) + '/track?hours=' + hours);
            if (!resp.ok) return [];
            return await resp.json();
        } catch(e) { return []; }
    }

    async function fetchAndRenderTrack(station) {
        var track = await fetchTrack(station, currentTrackHours);
        var statsEl = document.getElementById('track-stats');
        if (!statsEl) return;

        if (track.length < 2) {
            statsEl.innerHTML = '<div class="text-muted">Not enough position data for track display</div>';
            return;
        }

        // Calculate stats
        var totalDist = 0;
        var maxSpeed = 0;
        var speedSum = 0;
        var speedCount = 0;
        for (var i = 1; i < track.length; i++) {
            totalDist += haversine(track[i - 1].lat, track[i - 1].lon, track[i].lat, track[i].lon);
            if (track[i].speed != null) {
                maxSpeed = Math.max(maxSpeed, track[i].speed);
                speedSum += track[i].speed;
                speedCount++;
            }
        }
        var avgSpeed = speedCount > 0 ? speedSum / speedCount : 0;

        var startTime = parseTime(track[0].recorded_at);
        var endTime = parseTime(track[track.length - 1].recorded_at);
        var durationMs = endTime - startTime;
        var durationStr = '';
        if (durationMs > 3600000) {
            durationStr = (durationMs / 3600000).toFixed(1) + 'h';
        } else {
            durationStr = Math.floor(durationMs / 60000) + 'm';
        }

        statsEl.innerHTML = '<div class="track-stats">' +
            '<div class="track-stat"><span class="stat-value">' + track.length + '</span><span class="stat-label">Points</span></div>' +
            '<div class="track-stat"><span class="stat-value">' + totalDist.toFixed(1) + '</span><span class="stat-label">Miles</span></div>' +
            '<div class="track-stat"><span class="stat-value">' + avgSpeed.toFixed(0) + '</span><span class="stat-label">Avg mph</span></div>' +
            '<div class="track-stat"><span class="stat-value">' + maxSpeed.toFixed(0) + '</span><span class="stat-label">Max mph</span></div>' +
            '<div class="track-stat"><span class="stat-value">' + durationStr + '</span><span class="stat-label">Duration</span></div>' +
            '</div>';

        // Altitude chart — only create container when data exists
        if (altitudeChart) { altitudeChart.destroy(); altitudeChart = null; }
        var altContainer = document.getElementById('altitude-container');
        var altData = track.map(function(t) { return t.altitude; });
        if (altData.some(function(v) { return v != null; }) && typeof Chart !== 'undefined' && altContainer) {
            altContainer.innerHTML = '<div class="chart-container" style="margin-top:12px"><canvas id="chart-altitude"></canvas></div>';
            var times = track.map(function(t) { return parseTime(t.recorded_at); });
            var ctx = document.getElementById('chart-altitude');
            if (ctx) {
                altitudeChart = new Chart(ctx.getContext('2d'), {
                    type: 'line',
                    data: {
                        labels: times,
                        datasets: [{
                            label: 'Altitude',
                            data: altData,
                            borderColor: '#10b981',
                            backgroundColor: 'rgba(16,185,129,0.1)',
                            fill: true,
                            tension: 0.3,
                            pointRadius: 1,
                        }]
                    },
                    options: Object.assign({}, chartDefaults(), {
                        plugins: { legend: { display: false }, title: { display: true, text: 'Altitude (ft)', color: '#a1a1aa' } }
                    })
                });
            }
        }
    }

    async function fetchAndShowTrackOnMap(station) {
        var track = await fetchTrack(station, currentTrackHours);
        if (track.length < 2) return;

        var coordinates = track
            .filter(function(t) { return typeof t.lon === 'number' && typeof t.lat === 'number' && isFinite(t.lon) && isFinite(t.lat); })
            .map(function(t) { return [t.lon, t.lat]; });

        // Speed-colored segments
        var features = [];
        for (var i = 1; i < coordinates.length; i++) {
            var speed = track[i].speed || 0;
            var color = speedColor(speed);
            features.push({
                type: 'Feature',
                geometry: { type: 'LineString', coordinates: [coordinates[i - 1], coordinates[i]] },
                properties: { color: color, speed: speed },
            });
        }

        var geojson = { type: 'FeatureCollection', features: features };
        updateTracks(JSON.stringify(geojson));
        setTracksVisible(true);

        // Sync app track state and checkbox
        if (typeof syncTrackState === 'function') {
            syncTrackState(true);
        }

        // Fit map to track bounds
        if (coordinates.length > 0 && typeof fitToTrack === 'function') {
            fitToTrack(coordinates);
        }
    }

    function speedColor(speed) {
        if (speed < 5) return '#10b981';   // green (slow/stopped)
        if (speed < 30) return '#84cc16';  // lime
        if (speed < 60) return '#f59e0b';  // amber
        return '#ef4444';                   // red (fast)
    }

    function haversine(lat1, lon1, lat2, lon2) {
        var R = 3958.8; // miles
        var dLat = (lat2 - lat1) * Math.PI / 180;
        var dLon = (lon2 - lon1) * Math.PI / 180;
        var a = Math.sin(dLat / 2) * Math.sin(dLat / 2) +
            Math.cos(lat1 * Math.PI / 180) * Math.cos(lat2 * Math.PI / 180) *
            Math.sin(dLon / 2) * Math.sin(dLon / 2);
        return R * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
    }

    // === Packets Tab ===
    var packetSourceFilter = 'all';

    async function loadPacketsTab(station) {
        var panel = document.getElementById('tab-packets');
        if (!panel) return;

        panel.innerHTML = '<div class="packet-filter-btns">' +
            '<span class="text-muted" style="margin-right:8px">Show:</span>' +
            '<button class="range-btn active" data-filter="all">All</button>' +
            '<button class="range-btn" data-filter="tnc">RF Only</button>' +
            '<button class="range-btn" data-filter="aprs-is">NET Only</button>' +
            '</div>' +
            '<div id="station-packets-list"><div class="text-muted" style="padding:12px">Loading...</div></div>';

        packetSourceFilter = 'all';
        panel.querySelectorAll('.range-btn').forEach(function(btn) {
            btn.addEventListener('click', function() {
                panel.querySelectorAll('.range-btn').forEach(function(b) { b.classList.remove('active'); });
                btn.classList.add('active');
                packetSourceFilter = btn.dataset.filter;
                renderPacketsList(station._packets || []);
            });
        });

        fetchAndRenderPackets(station);
    }

    async function fetchAndRenderPackets(station) {
        var call = station.ssid > 0 ? station.callsign + '-' + station.ssid : station.callsign;
        try {
            var resp = await fetch('/api/stations/' + encodeURIComponent(call) + '/packets?limit=50');
            if (!resp.ok) return;
            station._packets = await resp.json();
            renderPacketsList(station._packets);
        } catch(e) { /* ignore */ }
    }

    function renderPacketsList(packets) {
        var el = document.getElementById('station-packets-list');
        if (!el) return;

        var filtered = packets;
        if (packetSourceFilter !== 'all') {
            filtered = packets.filter(function(p) { return p.source_type === packetSourceFilter; });
        }

        if (filtered.length === 0) {
            el.innerHTML = '<div class="text-muted" style="padding:12px">No packets</div>';
            return;
        }

        var html = '<table class="packet-table" style="font-size:12px"><thead><tr>' +
            '<th>Time</th><th>Type</th><th>Source</th><th>Path</th><th>Summary</th>' +
            '</tr></thead><tbody>';
        filtered.forEach(function(p) {
            html += '<tr class="packet-row">';
            html += '<td class="pkt-time">' + esc(formatTime(p.received_at)) + '</td>';
            html += '<td><span class="type-badge type-' + (p.packet_type || 'unknown').toLowerCase().replace(/[^a-z]/g, '') + '">' + esc(p.packet_type || '?') + '</span></td>';
            html += '<td>' + sourceBadge(p.source_type) + '</td>';
            html += '<td style="max-width:120px;overflow:hidden;text-overflow:ellipsis" title="' + esc(p.path || '') + '">' + esc(p.path || 'Direct') + '</td>';
            html += '<td class="pkt-summary">' + esc(p.summary || p.raw_info) + '</td>';
            html += '</tr>';
        });
        html += '</tbody></table>';

        el.innerHTML = html;
    }

    // Expose for app.js integration
    window.stationDetailInit = function() {
        // Close on backdrop click (use mousedown to avoid triggering during resize drag)
        var modal = document.getElementById('station-modal');
        if (modal) {
            modal.addEventListener('mousedown', function(e) {
                if (e.target === modal) closeStationModal();
            });
        }
        // Close button
        var closeBtn = document.getElementById('station-modal-close');
        if (closeBtn) {
            closeBtn.addEventListener('click', closeStationModal);
        }
        // Tab switching
        document.querySelectorAll('.modal-tab').forEach(function(tab) {
            tab.addEventListener('click', function() {
                switchTab(tab.dataset.tab);
            });
        });
    };

    // Chart.js adapter for time scale (if chart.js loaded)
    if (typeof Chart !== 'undefined') {
        // Configure Chart.js defaults for dark theme
        Chart.defaults.color = '#a1a1aa';
        Chart.defaults.borderColor = 'rgba(255,255,255,0.06)';
    }
})();

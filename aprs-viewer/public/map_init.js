// MapLibre GL JS integration for APRS Viewer
// Global functions called from app.js

var map = null;
var stationClickCallback = null;
var mapClickCallback = null;
var mapReady = false;
var pendingStations = null;
var pendingTracks = null;
var hoverPopup = null;
var pulseTimers = {};

// Dark basemap style using Protomaps PMTiles
function darkStyle(tileUrl) {
    return {
        version: 8,
        sources: {
            'protomaps': {
                type: 'vector',
                url: 'pmtiles://' + tileUrl,
                attribution: '&copy; OpenStreetMap'
            }
        },
        glyphs: 'https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf',
        layers: [
            {
                id: 'background',
                type: 'background',
                paint: { 'background-color': '#09090b' }
            },
            {
                id: 'water',
                type: 'fill',
                source: 'protomaps',
                'source-layer': 'water',
                paint: { 'fill-color': '#0c1323' }
            },
            {
                id: 'land',
                type: 'fill',
                source: 'protomaps',
                'source-layer': 'earth',
                paint: { 'fill-color': '#131316' }
            },
            {
                id: 'roads',
                type: 'line',
                source: 'protomaps',
                'source-layer': 'roads',
                paint: { 'line-color': '#1e1e21', 'line-width': 0.5 }
            },
            {
                id: 'boundaries',
                type: 'line',
                source: 'protomaps',
                'source-layer': 'boundaries',
                paint: { 'line-color': '#2a2a2e', 'line-width': 1, 'line-dasharray': [4, 4] }
            }
        ]
    };
}

// Dark theme paint overrides keyed by liberty layer ID
var darkOverrides = {
    'background':           { 'background-color': '#09090b' },
    'water':                { 'fill-color': '#0c1323' },
    'natural_earth':        '_hide_',
    'park':                 { 'fill-color': '#101012' },
    'park_outline':         { 'line-color': '#1a1a1e' },
    'landuse_residential':  { 'fill-color': '#0e0e10' },
    'landcover_wood':       { 'fill-color': '#0f100f' },
    'landcover_grass':      { 'fill-color': '#0f100f' },
    'landcover_ice':        { 'fill-color': '#101018' },
    'road_motorway':        { 'line-color': '#2e2e32' },
    'road_trunk':           { 'line-color': '#27272a' },
    'road_primary':         { 'line-color': '#27272a' },
    'road_secondary':       { 'line-color': '#222226' },
    'road_minor':           { 'line-color': '#1e1e21' },
    'road_service':         { 'line-color': '#1a1a1e' },
    'road_path':            { 'line-color': '#18181b' },
    'bridge_motorway':      { 'line-color': '#2e2e32' },
    'bridge_trunk':         { 'line-color': '#27272a' },
    'bridge_primary':       { 'line-color': '#27272a' },
    'bridge_secondary':     { 'line-color': '#222226' },
    'bridge_minor':         { 'line-color': '#1e1e21' },
    'tunnel_motorway':      { 'line-color': '#222226' },
    'tunnel_trunk':         { 'line-color': '#1e1e21' },
    'tunnel_primary':       { 'line-color': '#1e1e21' },
    'rail':                 { 'line-color': '#1a1a1e' },
    'rail_dash':            { 'line-color': '#222226' },
    'boundary_country':     { 'line-color': '#2a2a2e' },
    'boundary_state':       { 'line-color': '#222226' },
    'building':             { 'fill-color': '#131316' },
    'building_outline':     { 'line-color': '#1a1a1e' },
    'water_name':           { 'text-color': '#2a3a4a', 'text-halo-color': '#09090b' },
    'waterway_name':        { 'text-color': '#2a3a4a', 'text-halo-color': '#09090b' },
};

// Apply dark overrides directly to a style JSON object (pre-load)
function applyDarkToStyle(style) {
    if (!style || !style.layers) return style;

    for (var i = 0; i < style.layers.length; i++) {
        var layer = style.layers[i];
        var override = darkOverrides[layer.id];

        if (override === '_hide_') {
            if (!layer.layout) layer.layout = {};
            layer.layout.visibility = 'none';
            continue;
        }

        if (override) {
            if (!layer.paint) layer.paint = {};
            for (var prop in override) {
                layer.paint[prop] = override[prop];
            }
            continue;
        }

        // Generic darkening — only override simple string/color values,
        // skip layers with data-driven expressions to avoid null errors
        if (layer.type === 'fill' && layer.paint) {
            var fc = layer.paint['fill-color'];
            if (typeof fc === 'string') {
                layer.paint['fill-color'] = '#101012';
            }
        }
        if (layer.type === 'symbol' && layer.paint) {
            var tc = layer.paint['text-color'];
            if (typeof tc === 'string' || tc === undefined) {
                layer.paint['text-color'] = '#52525b';
                layer.paint['text-halo-color'] = '#09090b';
            }
        }
    }
    return style;
}

// Simple fallback style (no tiles at all)
function simpleStyle() {
    return {
        version: 8,
        sources: {},
        glyphs: 'https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf',
        layers: [
            {
                id: 'background',
                type: 'background',
                paint: { 'background-color': '#09090b' }
            }
        ]
    };
}

// Age-based opacity: 0-30min = 1.0, 30-120min fade, >120min = 0.25
function ageOpacity(ageMinutes) {
    if (ageMinutes <= 30) return 1.0;
    if (ageMinutes >= 120) return 0.25;
    return 1.0 - 0.75 * (ageMinutes - 30) / 90;
}

// Add APRS data layers (stations, tracks) to the map
function addAprsLayers() {
    // Register APRS icons
    if (typeof registerAprsIcons === 'function') {
        registerAprsIcons(map);
    }

    // Add station source (empty initially)
    map.addSource('aprs-stations', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] }
    });

    // Add track source
    map.addSource('aprs-tracks', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] }
    });

    // Pulse source — expanding rings for new packets
    map.addSource('aprs-pulse', {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] }
    });

    // Track lines layer — uses per-segment color from properties
    map.addLayer({
        id: 'tracks-line',
        type: 'line',
        source: 'aprs-tracks',
        layout: { visibility: 'none' },
        paint: {
            'line-color': ['coalesce', ['get', 'color'], '#4fc3f7'],
            'line-width': 2.5,
            'line-opacity': 0.7,
        }
    });

    // RF/NET ring — colored ring around stations based on heard_via
    map.addLayer({
        id: 'stations-ring',
        type: 'circle',
        source: 'aprs-stations',
        paint: {
            'circle-radius': [
                'interpolate', ['linear'], ['zoom'],
                4, ['match', ['get', 'stationType'], 'Weather', 8, 'Mobile', 7, 7],
                8, ['match', ['get', 'stationType'], 'Weather', 12, 'Mobile', 10, 11],
                12, ['match', ['get', 'stationType'], 'Weather', 15, 'Mobile', 13, 14],
                16, ['match', ['get', 'stationType'], 'Weather', 18, 'Mobile', 16, 17]
            ],
            'circle-color': 'transparent',
            'circle-stroke-width': 2,
            'circle-stroke-color': [
                'case',
                // RF heard = green ring
                ['!=', ['index-of', 'tnc', ['get', 'heardVia']], -1], '#10b981',
                // APRS-IS only = blue ring
                ['!=', ['index-of', 'aprs-is', ['get', 'heardVia']], -1], '#3b82f6',
                'transparent'
            ],
            'circle-opacity': [
                'interpolate', ['linear'], ['get', 'ageMinutes'],
                0, 1, 30, 1, 120, 0.25
            ],
            'circle-stroke-opacity': [
                'interpolate', ['linear'], ['get', 'ageMinutes'],
                0, 0.6, 30, 0.6, 120, 0.15
            ],
        }
    });

    // Station circles layer — fallback for stations without APRS icons
    map.addLayer({
        id: 'stations-circle',
        type: 'circle',
        source: 'aprs-stations',
        filter: ['!', ['has', 'hasIcon']],
        paint: {
            'circle-radius': [
                'interpolate', ['linear'], ['zoom'],
                4, ['match', ['get', 'stationType'], 'Weather', 6, 'Mobile', 5, 5],
                8, ['match', ['get', 'stationType'], 'Weather', 8, 'Mobile', 6, 7],
                12, ['match', ['get', 'stationType'], 'Weather', 10, 'Mobile', 8, 9],
                16, ['match', ['get', 'stationType'], 'Weather', 14, 'Mobile', 11, 12]
            ],
            'circle-color': [
                'match', ['get', 'stationType'],
                'Position', '#10b981',
                'Mobile', '#84cc16',
                'Weather', '#3b82f6',
                'Object', '#f59e0b',
                'Item', '#f59e0b',
                'Message', '#a855f7',
                '#22d3ee'
            ],
            'circle-stroke-width': [
                'case',
                ['get', 'selected'], 3,
                1
            ],
            'circle-stroke-color': [
                'case',
                ['get', 'selected'], '#6366f1',
                'rgba(255,255,255,0.5)'
            ],
            'circle-opacity': [
                'interpolate', ['linear'], ['get', 'ageMinutes'],
                0, 1, 30, 1, 120, 0.25
            ],
        }
    });

    // Station icon layer — APRS symbol icons for known symbols
    map.addLayer({
        id: 'stations-icon',
        type: 'symbol',
        source: 'aprs-stations',
        filter: ['has', 'hasIcon'],
        layout: {
            'icon-image': ['get', 'iconId'],
            'icon-size': [
                'interpolate', ['linear'], ['zoom'],
                4, 0.4,
                8, 0.55,
                12, 0.75,
                16, 0.9
            ],
            'icon-allow-overlap': true,
            'icon-ignore-placement': true,
        },
        paint: {
            'icon-opacity': [
                'interpolate', ['linear'], ['get', 'ageMinutes'],
                0, 1, 30, 1, 120, 0.25
            ],
        }
    });

    // Pulse ring layer for new packet animation
    map.addLayer({
        id: 'stations-pulse',
        type: 'circle',
        source: 'aprs-pulse',
        paint: {
            'circle-radius': 20,
            'circle-color': 'transparent',
            'circle-stroke-width': 2,
            'circle-stroke-color': '#6366f1',
            'circle-opacity': 0,
            'circle-stroke-opacity': ['get', 'opacity'],
        }
    });

    // Station labels — show callsign, plus weather summary for WX stations
    map.addLayer({
        id: 'stations-label',
        type: 'symbol',
        source: 'aprs-stations',
        layout: {
            'text-field': [
                'case',
                ['!=', ['get', 'wxLabel'], ''],
                ['concat', ['get', 'callsign'], '\n', ['get', 'wxLabel']],
                ['get', 'callsign']
            ],
            'text-font': ['Noto Sans Regular'],
            'text-size': 11,
            'text-offset': [0, 1.5],
            'text-anchor': 'top',
            'text-optional': true,
        },
        paint: {
            'text-color': '#ccc',
            'text-halo-color': '#000',
            'text-halo-width': 1,
            'text-opacity': [
                'interpolate', ['linear'], ['get', 'ageMinutes'],
                0, 1, 30, 1, 120, 0.3
            ],
        }
    });

    // Wind direction arrows for WX stations
    map.addLayer({
        id: 'stations-wind',
        type: 'symbol',
        source: 'aprs-stations',
        filter: ['get', 'hasWind'],
        layout: {
            'text-field': '\u2191',  // Unicode up arrow
            'text-size': 16,
            'text-font': ['Noto Sans Regular'],
            'text-rotation-alignment': 'map',
            'text-rotate': ['get', 'windDirection'],
            'text-offset': [1.5, 0],
            'text-allow-overlap': true,
            'text-ignore-placement': true,
        },
        paint: {
            'text-color': '#60a5fa',
            'text-opacity': [
                'interpolate', ['linear'], ['get', 'ageMinutes'],
                0, 0.8, 30, 0.8, 120, 0.2
            ],
        }
    });

    // Click handlers for both circle and icon layers
    var clickLayers = ['stations-circle', 'stations-icon'];
    clickLayers.forEach(function(layerId) {
        map.on('click', layerId, function(e) {
            if (e.features.length > 0 && stationClickCallback) {
                stationClickCallback(e.features[0].properties.callsign);
            }
        });
    });

    // Click on empty map
    map.on('click', function(e) {
        var features = map.queryRenderedFeatures(e.point, { layers: clickLayers });
        if (features.length === 0 && mapClickCallback) {
            mapClickCallback(e.lngLat.lng, e.lngLat.lat);
        }
    });

    // Hover popup
    hoverPopup = new maplibregl.Popup({
        closeButton: false,
        closeOnClick: false,
        className: 'station-popup',
        offset: 12,
    });

    var hoverLayers = ['stations-circle', 'stations-icon'];
    hoverLayers.forEach(function(layerId) {
        map.on('mouseenter', layerId, function(e) {
            map.getCanvas().style.cursor = 'pointer';
            if (e.features.length === 0) return;

            var props = e.features[0].properties;
            var coords = e.features[0].geometry.coordinates.slice();

            // Build popup content
            var symDesc = '';
            if (typeof getSymbolDescription === 'function' && props.symbolTable && props.symbolCode) {
                symDesc = getSymbolDescription(props.symbolTable, props.symbolCode);
            }
            var srcHtml = '';
            var hv = props.heardVia || '';
            if (hv.indexOf('tnc') >= 0) srcHtml += '<span class="source-badge source-rf" style="font-size:10px">RF</span> ';
            if (hv.indexOf('aprs-is') >= 0) srcHtml += '<span class="source-badge source-net" style="font-size:10px">NET</span>';

            var html = '<div class="popup-content">' +
                '<div class="popup-call">' + escHtml(props.callsign) + '</div>' +
                '<div class="popup-type">' + escHtml(props.stationType);
            if (symDesc && symDesc !== 'Unknown') html += ' &middot; ' + escHtml(symDesc);
            html += '</div>';
            if (srcHtml) html += '<div class="popup-source">' + srcHtml + '</div>';

            // Age
            if (props.ageMinutes !== undefined) {
                var age = props.ageMinutes;
                var ageStr = age < 1 ? 'just now' :
                    age < 60 ? Math.floor(age) + 'm ago' :
                    age < 1440 ? Math.floor(age / 60) + 'h ago' :
                    Math.floor(age / 1440) + 'd ago';
                html += '<div class="popup-age">' + ageStr + '</div>';
            }
            html += '</div>';

            hoverPopup.setLngLat(coords).setHTML(html).addTo(map);
        });

        map.on('mouseleave', layerId, function() {
            map.getCanvas().style.cursor = '';
            hoverPopup.remove();
        });
    });

    // Mark map as ready and apply any buffered data
    mapReady = true;
    if (pendingStations) {
        try { map.getSource('aprs-stations').setData(pendingStations); } catch(e) {}
        pendingStations = null;
    }
    if (pendingTracks) {
        try { map.getSource('aprs-tracks').setData(pendingTracks); } catch(e) {}
        pendingTracks = null;
    }
}

function escHtml(s) {
    if (s == null) return '';
    return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function createMap(containerId, lng, lat, zoom, style) {
    try {
        map = new maplibregl.Map({
            container: containerId,
            style: style,
            center: [lng, lat],
            zoom: zoom,
        });
    } catch (e) {
        console.error('Failed to create map:', e);
        return;
    }

    map.on('error', function(e) {
        console.error('Map error:', e.error || e);
    });

    map.on('load', addAprsLayers);
}

function initMap(containerId, lng, lat, zoom, tileUrl, darkMode) {
    if (typeof maplibregl === 'undefined') {
        console.warn('MapLibre GL JS not loaded, using placeholder');
        return;
    }

    // Register PMTiles protocol if available
    var hasPmtiles = false;
    if (typeof pmtiles !== 'undefined') {
        var protocol = new pmtiles.Protocol();
        maplibregl.addProtocol('pmtiles', protocol.tile);
        hasPmtiles = true;
    }

    // Use PMTiles if local file available
    if (hasPmtiles && tileUrl) {
        createMap(containerId, lng, lat, zoom, darkStyle(tileUrl));
        return;
    }

    // Fetch OpenFreeMap liberty style, apply dark overrides, then create map
    fetch('https://tiles.openfreemap.org/styles/liberty')
        .then(function(resp) {
            if (!resp.ok) throw new Error('Failed to fetch liberty style: ' + resp.status);
            return resp.json();
        })
        .then(function(style) {
            applyDarkToStyle(style);
            createMap(containerId, lng, lat, zoom, style);
        })
        .catch(function(err) {
            console.error('Failed to load liberty style, using fallback:', err);
            createMap(containerId, lng, lat, zoom, simpleStyle());
        });
}

function updateStations(geojsonStr) {
    if (!map) return;
    var data = JSON.parse(geojsonStr);

    // Compute iconId and hasIcon for each feature
    for (var i = 0; i < data.features.length; i++) {
        var p = data.features[i].properties;
        if (typeof getSymbolIconId === 'function' && p.symbolTable && p.symbolCode) {
            var iconId = getSymbolIconId(p.symbolTable, p.symbolCode);
            if (iconId && map.hasImage(iconId)) {
                p.iconId = iconId;
                p.hasIcon = true;
            }
            // Non-prominent symbols: no hasIcon → falls through to
            // stations-circle layer (colored dot based on station type)
        }
    }

    if (!mapReady) {
        pendingStations = data;
        return;
    }
    try {
        var source = map.getSource('aprs-stations');
        if (source) {
            source.setData(data);
        }
    } catch (e) {
        console.error('Failed to update stations:', e);
    }
}

function updateTracks(geojsonStr) {
    if (!map) return;
    var data = JSON.parse(geojsonStr);
    if (!mapReady) {
        pendingTracks = data;
        return;
    }
    try {
        var source = map.getSource('aprs-tracks');
        if (source) {
            source.setData(data);
        }
    } catch (e) {
        console.error('Failed to update tracks:', e);
    }
}

function flyTo(lng, lat, zoom) {
    if (map) {
        // Use current zoom if already closer than requested
        var currentZoom = map.getZoom();
        var targetZoom = (currentZoom > zoom) ? currentZoom : zoom;
        map.flyTo({ center: [lng, lat], zoom: targetZoom, duration: 1000 });
    }
}

function fitToTrack(coordinates) {
    if (!map || !coordinates || coordinates.length === 0) return;
    var bounds = new maplibregl.LngLatBounds();
    for (var i = 0; i < coordinates.length; i++) {
        bounds.extend(coordinates[i]);
    }
    map.fitBounds(bounds, { padding: 60, duration: 1000 });
}

function onStationClick(callback) {
    stationClickCallback = callback;
}

function onMapClick(callback) {
    mapClickCallback = callback;
}

function destroyMap() {
    if (map) {
        map.remove();
        map = null;
    }
}

function setTracksVisible(visible) {
    if (!map) return;
    var layer = map.getLayer('tracks-line');
    if (layer) {
        map.setLayoutProperty('tracks-line', 'visibility', visible ? 'visible' : 'none');
    }
}

function clearTracks() {
    if (!map) return;
    try {
        var source = map.getSource('aprs-tracks');
        if (source) {
            source.setData({ type: 'FeatureCollection', features: [] });
        }
    } catch(e) {}
    setTracksVisible(false);
}

function getMapCenter() {
    if (!map) return "";
    var center = map.getCenter();
    return JSON.stringify({ lng: center.lng, lat: center.lat, zoom: map.getZoom() });
}

// Pulse animation: briefly show an expanding ring on a station's position
function pulseStation(station) {
    if (!map || !mapReady) return;
    if (typeof station.lat !== 'number' || typeof station.lon !== 'number') return;

    var key = station.callsign + (station.ssid > 0 ? '-' + station.ssid : '');

    // Cancel previous pulse for this station
    if (pulseTimers[key]) {
        clearInterval(pulseTimers[key]);
        delete pulseTimers[key];
    }

    var startTime = Date.now();
    var duration = 800; // ms

    function animate() {
        var elapsed = Date.now() - startTime;
        if (elapsed > duration) {
            clearInterval(pulseTimers[key]);
            delete pulseTimers[key];
            // Clear pulse source
            try {
                var s = map.getSource('aprs-pulse');
                if (s) s.setData({ type: 'FeatureCollection', features: [] });
            } catch(e) {}
            return;
        }

        var progress = elapsed / duration;
        var radius = 8 + 24 * progress;
        var opacity = 0.7 * (1 - progress);

        try {
            var s = map.getSource('aprs-pulse');
            if (s) {
                s.setData({
                    type: 'FeatureCollection',
                    features: [{
                        type: 'Feature',
                        geometry: { type: 'Point', coordinates: [station.lon, station.lat] },
                        properties: { opacity: opacity },
                    }]
                });
            }
            map.setPaintProperty('stations-pulse', 'circle-radius', radius);
        } catch(e) {}
    }

    pulseTimers[key] = setInterval(animate, 30);
    animate();
}

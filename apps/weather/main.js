// alloyOS App: Weather (/apps/weather/main.js)
// Executed inside a capability-scoped alloyos-js isolate by alloyos-mcp.

const CONFIG_PATH = "/var/weather/config.json";
const CACHE_PATH = "/var/weather/cache.json";

function getConfig() {
    let raw = AlloyOS.fs.read(CONFIG_PATH);
    if (!raw) {
        return {
            default_city: "San Francisco",
            units: "celsius"
        };
    }
    let cfg = JSON.parse(raw);
    return {
        default_city: cfg.default_city || "San Francisco",
        units: cfg.units || "celsius"
    };
}

function saveConfig(cfg) {
    AlloyOS.fs.write(CONFIG_PATH, JSON.stringify(cfg));
}

function getCachedWeather() {
    let raw = AlloyOS.fs.read(CACHE_PATH);
    if (raw) {
        let parsed = JSON.parse(raw);
        if (parsed && parsed.city) {
            return parsed;
        }
    }
    return {
        city: "San Francisco",
        condition: "Clear",
        temp_c: 20,
        temp_f: 68,
        humidity: 50,
        wind_kmh: 10,
        forecast: [],
        updated_at: "initial"
    };
}

function saveCachedWeather(data) {
    AlloyOS.fs.write(CACHE_PATH, JSON.stringify(data));
}

function fetchWeatherFromNetwork(cityName) {
    let city = cityName || getConfig().default_city;
    let queryCity = city.trim().split(" ").join("+");
    let url = "https://wttr.in/" + queryCity + "?format=j1";

    let raw = AlloyOS.net.fetch(url);
    let data = JSON.parse(raw);

    let current = {};
    if (data.current_condition && data.current_condition.length > 0) {
        current = data.current_condition[0];
    }

    let desc = "Clear";
    if (current.weatherDesc && current.weatherDesc.length > 0 && current.weatherDesc[0].value) {
        desc = current.weatherDesc[0].value;
    }

    let tempC = parseInt(current.temp_C || "0");
    let tempF = parseInt(current.temp_F || "0");
    let humidity = parseInt(current.humidity || "0");
    let windKmh = parseInt(current.windspeedKmph || "0");

    let forecastList = [];
    if (data.weather && data.weather.length > 0) {
        for (let i = 0; i < data.weather.length; i = i + 1) {
            let dayItem = data.weather[i];
            let dayDesc = desc;
            if (dayItem.hourly && dayItem.hourly.length > 0 && dayItem.hourly[0].weatherDesc && dayItem.hourly[0].weatherDesc.length > 0) {
                dayDesc = dayItem.hourly[0].weatherDesc[0].value;
            }
            forecastList.push({
                date: dayItem.date || ("Day " + (i + 1)),
                condition: dayDesc,
                high_c: parseInt(dayItem.maxtempC || "0"),
                low_c: parseInt(dayItem.mintempC || "0")
            });
        }
    }

    let result = {
        city: city,
        condition: desc,
        temp_c: tempC,
        temp_f: tempF,
        humidity: humidity,
        wind_kmh: windKmh,
        forecast: forecastList,
        updated_at: "network"
    };

    saveCachedWeather(result);
    return result;
}

function callTool(toolName, args) {
    if (toolName === "weather.current") {
        let city = (args && args.city) || getConfig().default_city;
        let units = (args && args.units) || getConfig().units;
        let weather = fetchWeatherFromNetwork(city);

        let displayTemp = weather.temp_c + "°C";
        if (units === "fahrenheit") {
            displayTemp = weather.temp_f + "°F";
        }

        return JSON.stringify({
            city: weather.city,
            condition: weather.condition,
            temperature: displayTemp,
            temp_c: weather.temp_c,
            temp_f: weather.temp_f,
            humidity_percent: weather.humidity,
            wind_kmh: weather.wind_kmh
        });
    }

    if (toolName === "weather.forecast") {
        let city = (args && args.city) || getConfig().default_city;
        let days = (args && args.days) || 3;
        let weather = fetchWeatherFromNetwork(city);

        let forecast = weather.forecast || [];
        if (days < forecast.length) {
            forecast = forecast.slice(0, days);
        }

        return JSON.stringify({
            city: weather.city,
            days: forecast.length,
            forecast: forecast
        });
    }

    if (toolName === "weather.set_location") {
        let city = (args && args.city) || "San Francisco";
        let cfg = getConfig();
        cfg.default_city = city;
        saveConfig(cfg);

        let newWeather = fetchWeatherFromNetwork(city);

        return JSON.stringify({
            status: "updated",
            default_city: city,
            current_temperature: newWeather.temp_c + "°C",
            condition: newWeather.condition
        });
    }

    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    if (uri === "mcp://weather/current") {
        let current = getCachedWeather();
        return JSON.stringify(current);
    }

    if (uri === "mcp://weather/config") {
        let cfg = getConfig();
        return JSON.stringify(cfg);
    }

    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "weather.card") {
        let weather = getCachedWeather();
        let cfg = getConfig();

        let tempStr = weather.temp_c + "°C";
        if (cfg.units === "fahrenheit") {
            tempStr = weather.temp_f + "°F";
        }

        let items = [
            {
                id: "condition",
                title: weather.city + " · " + tempStr,
                subtitle: weather.condition,
                badge: weather.condition,
                status: "live"
            },
            {
                id: "humidity",
                title: "Humidity: " + weather.humidity + "%",
                subtitle: "Wind: " + weather.wind_kmh + " km/h",
                badge: "Atmosphere",
                status: "normal"
            }
        ];

        return JSON.stringify({
            tile_id: "weather.card",
            title: "Weather",
            subtitle: weather.city + " · " + tempStr,
            refresh_resource_uri: "mcp://weather/current",
            items: items,
            actions: [
                {
                    id: "refresh",
                    label: "Refresh Weather",
                    tool_name: "weather.current",
                    default_args: {}
                }
            ]
        });
    }

    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}

function getPrompt(name, args) {
    if (name === "weather.briefing") {
        let city = (args && args.city) || getConfig().default_city;
        return "Please give a short daily weather briefing and outfit advice for " + city;
    }
    return "Weather prompt: " + name;
}

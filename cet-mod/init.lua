-- g13-hud: Cyberpunk 2077's own numbers on a Logitech G13 screen.
--
-- This writes one small JSON file, about five times a second, and does nothing else. The G13
-- side reads it with the driver's `json:` data source, so the screen is designed and drawn on
-- the Linux side: this file is data, not a picture. Run G13Probe() in the CET console to see
-- what this particular game build offers.
--
-- Verified against mods that already run in this game (the calls are theirs):
--   health/stamina/level   Game.GetStatsSystem():GetStatValue(entityId, 'Health')
--   tracked objective      Game.GetJournalManager():GetTrackedEntry()
--   player position        Game.GetPlayer():GetWorldPosition() / GetWorldForward()
--   loc keys to text       GetLocalizedText(key)
-- Unverified calls are tried defensively and simply come back empty: the game build decides.
--
-- Requires CET 1.37+ (written against 1.37.1). Install to
--   <game>/bin/x64/plugins/cyber_engine_tweaks/mods/g13-hud/init.lua

local UPDATE_SECONDS = 0.2
local OUTPUT_FILE = "hud.json"      -- inside this mod's folder, which is all the sandbox allows
local VERSION = "1.0"

local elapsed = 0
local now_seconds = 0

local function number_from(fn)
    local ok, value = pcall(fn)
    if ok and type(value) == "number" and value == value then   -- reject NaN
        return value
    end
    return nil
end

-- A loc key to display text, or the key itself if it is not one.
local function localized(value)
    if type(value) ~= "string" or value == "" then
        return nil
    end
    local ok, text = pcall(GetLocalizedText, value)
    if ok and type(text) == "string" and text ~= "" then
        return text
    end
    return value
end

local function entry_text(entry)
    if not entry then
        return nil
    end
    for _, method in ipairs({ "GetTitle", "GetDescription" }) do
        local ok, value = pcall(function() return entry[method](entry) end)
        if ok then
            local text = localized(value)
            if text then
                return text
            end
        end
    end
    return nil
end

local function tracked()
    local state = {}
    local ok, entry = pcall(function() return Game.GetJournalManager():GetTrackedEntry() end)
    if not ok or not entry then
        return state
    end

    local quest
    pcall(function()
        local phase = Game.GetJournalManager():GetParentEntry(entry)
        if phase then
            quest = Game.GetJournalManager():GetParentEntry(phase)
        end
    end)

    state.objective = entry_text(entry)
    state.quest = entry_text(quest)
    pcall(function() state.objective_id = tostring(entry.id) end)
    pcall(function() state.quest_id = quest and tostring(quest.id) end)
    return state
end

local function weapon()
    local state = {}
    local ok, held = pcall(function() return Game.GetPlayer():GetActiveWeapon() end)
    if not ok or not held then
        return state
    end

    pcall(function() state.weapon = localized(held:GetName()) end)

    -- Which of these a build answers varies; whichever does is used.
    local candidates = {
        ammo = "GetMagazineAmmoCount",
        ammo_total = "GetAmmoCount",
        ammo_max = "GetMaxAmmoCount",
    }
    for key, method in pairs(candidates) do
        local value = number_from(function() return held[method](held) end)
        if value then
            state[key] = value
        end
    end
    return state
end

-- --- where you are, and where you are going ----------------------------------------------

-- The mappins the map knows about, with their positions: the player's own pin included. Held
-- for a couple of seconds, because walking the list five times a second is pointless work.
local MAPPIN_SECONDS = 2.0
local mappins = {}
local mappins_at = -MAPPIN_SECONDS

local function mappin_variant(mappin)
    local ok, value = pcall(function() return mappin:GetVariant() end)
    if not ok or value == nil then
        value = mappin.variant
    end
    return value
end

local function mappin_position(mappin)
    local ok, value = pcall(function() return mappin:GetWorldPosition() end)
    if not ok or value == nil then
        value = mappin.worldPosition
    end
    return value
end

local function mappin_name(mappin)
    for _, accessor in ipairs({ "GetName", "GetPointDisplayName", "GetDisplayName" }) do
        local ok, value = pcall(function() return mappin[accessor](mappin) end)
        if ok then
            local text = localized(value)
            if text and text ~= "" and not text:match("^LocKey") then
                return text
            end
        end
    end
    local ok, value = pcall(function() return mappin.name end)
    if ok then
        return localized(value)
    end
    return nil
end

local function refresh_mappins(now)
    if now - mappins_at < MAPPIN_SECONDS then
        return
    end
    mappins_at = now
    local ok, list = pcall(function()
        return Game.GetMappinSystem():GetMappins(gamemappinsMappinTargetType.Map)
    end)
    if not ok or type(list) ~= "table" then
        return
    end
    local collected = {}
    for _, mappin in ipairs(list) do
        local position = mappin_position(mappin)
        if position then
            collected[#collected + 1] = {
                variant = tostring(mappin_variant(mappin)),
                name = mappin_name(mappin),
                x = position.x, y = position.y, z = position.z,
            }
        end
    end
    if #collected > 0 then
        mappins = collected
    end
end

-- The player's map pin. The variant is compared by name as well as by identity, because the
-- enum table can differ between builds and an identity test alone then silently never matches.
local PIN_VARIANT = "CustomPosition"
local PIN_UNITS_PER_METRE = 100

local function is_pin(variant)
    if variant == nil or variant == "nil" then
        return false
    end
    local text = tostring(variant)
    local wanted = tostring(gamedataMappinVariant and gamedataMappinVariant.CustomPositionVariant)
    return text == wanted or text:find(PIN_VARIANT, 1, true) ~= nil
end

-- Bearing and distance from the player to a point, in the same convention as the heading:
-- 0 degrees is north, 90 is east. Nothing here reads the game's own route, so this is the
-- straight line to the pin, not the road.
local function towards(origin, target)
    local dx = target.x - origin.x
    local dy = target.y - origin.y
    local distance = math.sqrt(dx * dx + dy * dy)
    return {
        bearing = math.floor((math.deg(math.atan(dx, dy)) + 360) % 360),
        distance = math.floor(distance / PIN_UNITS_PER_METRE + 0.5),
        raw = math.floor(distance),
    }
end

local function destination(player_position, heading)
    local state = {}
    if not player_position then
        return state
    end
    local best
    for _, mappin in ipairs(mappins) do
        if is_pin(mappin.variant) then
            local distance = (mappin.x - player_position.x) ^ 2 + (mappin.y - player_position.y) ^ 2
            if not best or distance < best.distance then
                best = { mappin = mappin, distance = distance }
            end
        end
    end
    if not best then
        return state
    end

    local line = towards(player_position, best.mappin)
    state.pin_bearing = line.bearing
    state.pin_distance = line.distance
    state.pin_distance_raw = line.raw
    if heading then
        -- Where the pin is relative to where you are facing: 0 is straight ahead, 90 to the
        -- right. This is what the arrow on the screen points along.
        state.pin_relative = (line.bearing - heading + 360) % 360
    end
    return state
end

-- The nearest named place, for orientation when there is no pin: "near Megabuilding H10".
local function nearest_named(player_position)
    if not player_position then
        return nil
    end
    local best
    for _, mappin in ipairs(mappins) do
        if mappin.name then
            local distance = (mappin.x - player_position.x) ^ 2 + (mappin.y - player_position.y) ^ 2
            if not best or distance < best.distance then
                best = { name = mappin.name, distance = distance }
            end
        end
    end
    return best and best.name or nil
end

-- The district you are standing in, from the PreventionSystem's own idea of it.
local function area()
    local state = {}
    local ok, district_id = pcall(function()
        local prevention = Game.GetScriptableSystemsContainer():Get("PreventionSystem")
        return prevention.districtManager:GetCurrentDistrict():GetDistrictID()
    end)
    if not ok or district_id == nil then
        return state
    end
    pcall(function()
        local record = TweakDBInterface.GetDistrictRecord(district_id)
        state.district = localized(record:LocalizedName())
    end)
    return state
end

local function collect()
    local state = {
        mod = "g13-hud",
        version = VERSION,
        updated = os.date("%H:%M:%S"),
        health = number_from(function()
            return Game.GetStatsSystem():GetStatValue(Game.GetPlayer():GetEntityID(), "Health")
        end),
        stamina = number_from(function()
            return Game.GetStatsSystem():GetStatValue(Game.GetPlayer():GetEntityID(), "Stamina")
        end),
        level = number_from(function()
            return Game.GetStatsSystem():GetStatValue(Game.GetPlayer():GetEntityID(), "Level")
        end),
        streetcred = number_from(function()
            return Game.GetStatsSystem():GetStatValue(Game.GetPlayer():GetEntityID(), "StreetCred")
        end),
    }

    local player = Game.GetPlayer()
    local position
    pcall(function() position = player:GetWorldPosition() end)
    if position then
        state.x, state.y, state.z = position.x, position.y, position.z
    end

    -- Heading, in degrees, 0 = north and 90 = east, which is what the arrow expects. The
    -- axis convention is worth a glance in game; it is one line to flip if it reads backwards.
    local forward
    pcall(function() forward = player:GetWorldForward() end)
    if forward then
        state.heading = math.floor((math.deg(math.atan(forward.x, forward.y)) + 360) % 360)
    end

    for key, value in pairs(tracked()) do
        state[key] = value
    end
    for key, value in pairs(weapon()) do
        state[key] = value
    end

    -- Where you are, what you are pointed at, and what is nearby. The distance to a pin is a
    -- straight line: the game's own route is not readable, so this is as the crow flies.
    refresh_mappins(now_seconds)
    for key, value in pairs(destination(position, state.heading)) do
        state[key] = value
    end
    for key, value in pairs(area()) do
        state[key] = value
    end
    local nearby = nearest_named(position)
    if nearby then
        state.near = nearby
    end

    -- One ready-made line for a small screen, so the applet does not have to choose between
    -- fields it cannot test for itself: the pin when there is one, the area when there is not.
    if state.pin_distance then
        state.route = string.format("PIN %dm  %s", state.pin_distance, state.district or "")
    elseif nearby then
        state.route = string.format("%s  %s", state.district or "", nearby)
    else
        state.route = state.district
    end
    return state
end

local function write(state)
    local ok, text = pcall(json.encode, state)
    if not ok or type(text) ~= "string" then
        return
    end

    -- Written beside the real file and renamed over it, so the reader never sees half of it.
    -- If the rename is not allowed, a direct write is good enough: the reader ignores
    -- unreadable JSON rather than failing.
    local renamed = pcall(function()
        local file = io.open(OUTPUT_FILE .. ".tmp", "w")
        if not file then
            return false
        end
        file:write(text)
        file:close()
        os.remove(OUTPUT_FILE)
        os.rename(OUTPUT_FILE .. ".tmp", OUTPUT_FILE)
        return true
    end)

    if not renamed then
        pcall(function()
            local file = io.open(OUTPUT_FILE, "w")
            if file then
                file:write(text)
                file:close()
            end
        end)
    end
end

-- A console helper: G13Probe() lists what this build answers, so a missing field can be
-- tracked down in one session rather than guessed at.
function G13Probe()
    print("[g13-hud] probe, version " .. VERSION)
    local player = Game.GetPlayer()
    local methods = {
        "GetActiveWeapon", "GetWorldPosition", "GetWorldForward", "GetQuickSlotsManager",
    }
    for _, method in ipairs(methods) do
        local ok, value = pcall(function() return player[method](player) end)
        print(string.format("  player:%s -> %s", method, ok and tostring(value) or "refused"))
    end

    local held
    pcall(function() held = player:GetActiveWeapon() end)
    if held then
        for _, method in ipairs({ "GetName", "GetMagazineAmmoCount", "GetAmmoCount",
                                  "GetMaxAmmoCount", "GetAmmoLeftCount" }) do
            local ok, value = pcall(function() return held[method](held) end)
            print(string.format("  weapon:%s -> %s", method, ok and tostring(value) or "refused"))
        end
    end

    for _, name in ipairs({ "Health", "Stamina", "Level", "StreetCred" }) do
        local value = number_from(function()
            return Game.GetStatsSystem():GetStatValue(player:GetEntityID(), name)
        end)
        print(string.format("  stat %-12s -> %s", name, tostring(value)))
    end

    local entry
    pcall(function() entry = Game.GetJournalManager():GetTrackedEntry() end)
    print("  tracked entry -> " .. tostring(entry ~= nil))
    if entry then
        for _, method in ipairs({ "GetTitle", "GetDescription", "GetMappinPath", "GetPosition" }) do
            local ok, value = pcall(function() return entry[method](entry) end)
            print(string.format("  entry:%s -> %s", method, ok and tostring(value) or "refused"))
        end
    end
    -- The map's own list: the player's pin is one of these, and this is how to tell which one.
    local position_of_player
    pcall(function() position_of_player = player:GetWorldPosition() end)
    local listed, list = pcall(function()
        return Game.GetMappinSystem():GetMappins(gamemappinsMappinTargetType.Map)
    end)
    if listed and type(list) == "table" then
        print(string.format("  map mappins: %d", #list))
        local samples = {}
        local counts = {}
        for _, mappin in ipairs(list) do
            local variant = tostring(mappin_variant(mappin))
            counts[variant] = (counts[variant] or 0) + 1
            local position = mappin_position(mappin)
            local distance = -1
            if position and position_of_player then
                distance = math.floor(math.sqrt((position.x - position_of_player.x) ^ 2
                                                + (position.y - position_of_player.y) ^ 2))
            end
            samples[#samples + 1] = {
                variant = variant, name = mappin_name(mappin), distance = distance,
            }
        end
        table.sort(samples, function(first, second) return first.distance < second.distance end)
        print("  nearest mappins (units from you):")
        for index = 1, math.min(#samples, 8) do
            print(string.format("    %8d  %-40s %s", samples[index].distance,
                                samples[index].variant, samples[index].name or ""))
        end
        print("  variants present:")
        for variant, count in pairs(counts) do
            print(string.format("    %3d  %s", count, variant))
        end
    else
        print("  map mappins -> refused")
    end

    -- The district you are in, and the sub-districts its record knows about.
    pcall(function()
        local prevention = Game.GetScriptableSystemsContainer():Get("PreventionSystem")
        local district_id = prevention.districtManager:GetCurrentDistrict():GetDistrictID()
        local record = TweakDBInterface.GetDistrictRecord(district_id)
        print("  district -> " .. tostring(record:LocalizedName()))
        local subs
        local read = pcall(function() subs = record:SubDistrict() end)
        if not read then
            pcall(function() subs = record.SubDistrict end)
        end
        for _, sub in ipairs(subs or {}) do
            print("    subdistrict -> " .. tostring(sub))
        end
    end)

    print("[g13-hud] end of probe")
end

registerForEvent("onInit", function()
    print(string.format("[g13-hud] v%s: writing %s every %.2fs", VERSION, OUTPUT_FILE,
                        UPDATE_SECONDS))
    write(collect())
end)

registerForEvent("onUpdate", function(delta)
    elapsed = elapsed + (delta or 0)
    now_seconds = now_seconds + (delta or 0)
    if elapsed < UPDATE_SECONDS then
        return
    end
    elapsed = 0
    write(collect())
end)

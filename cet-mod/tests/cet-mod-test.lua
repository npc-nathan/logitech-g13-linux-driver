-- A stand-in for Cyberpunk 2077, so the g13-hud mod's logic can be tested without the game.
--
-- It provides the handful of globals CET gives a mod - Game, GetLocalizedText, json, io,
-- registerForEvent - with values that are easy to assert on, loads the mod, fires the events
-- CET would fire, and checks the file it wrote. tests/cet-mod-test.py runs it and then checks
-- the same file as JSON, which is what the Linux side will read.
--
-- The stubs deliberately include a build that *refuses* the weapon calls: the mod has to
-- survive a game build that has no ammo answer at all.

local function check(what, expected, actual)
    local ok = expected == actual
    if not ok then
        FAILURES = (FAILURES or 0) + 1
    end
    print(string.format("%-56s %-22s %s", what, "-> " .. tostring(actual),
                        ok and "ok" or ("FAIL (expected " .. tostring(expected) .. ")")))
end

local function truthy(what, actual)
    check(what, true, actual and true or false)
end

-- --- the game, as far as this mod is concerned -------------------------------------------

local REFUSE_WEAPON = os.getenv("G13_TEST_REFUSE_WEAPON") == "1"
local FORWARD = { x = tonumber(os.getenv("G13_TEST_FORWARD_X") or "0"),
                  y = tonumber(os.getenv("G13_TEST_FORWARD_Y") or "1"),
                  z = 0 }

local player = {
    GetEntityID = function(self) return 12345 end,
    GetWorldPosition = function(self) return { x = 100.5, y = -200.25, z = 8.0, w = 1 } end,
    GetWorldForward = function(self) return FORWARD end,
    GetActiveWeapon = function(self)
        if REFUSE_WEAPON then
            error("this build has no active weapon")
        end
        return {
            GetName = function(self) return "LocKey#77" end,
            GetMagazineAmmoCount = function(self) return 24 end,
            GetAmmoCount = function(self) return 180 end,
            GetMaxAmmoCount = function(self) return 24 end,
        }
    end,
}

local stats = { Health = 87, Stamina = 61, Level = 32, StreetCred = 50 }

local quest = { id = "q005_rogue" }
quest.GetTitle = function(self) return "LocKey#2001" end

-- The real hierarchy is three deep: the tracked entry is an objective, whose parent is a
-- phase, whose parent is the quest. The mod walks all three.
local phase = { id = "q005_rogue_phase" }

local objective = { id = "q005_rogue_obj_2" }
objective.GetTitle = function(self) return "LocKey#1234" end

Game = {
    GetPlayer = function() return player end,
    GetStatsSystem = function()
        return {
            GetStatValue = function(self, _id, name) return stats[name] end,
        }
    end,
    GetJournalManager = function()
        return {
            GetTrackedEntry = function(self) return objective end,
            GetParentEntry = function(self, entry)
                if entry == objective then return phase end
                if entry == phase then return quest end
                return nil
            end,
        }
    end,
    -- The map, with the player's own pin on it. The pin is the mappin whose variant matches
    -- the game's CustomPositionVariant, whatever build this is.
    GetMappinSystem = function()
        return {
            GetMappins = function(self, _target)
                return {
                    { GetVariant = function() return gamedataMappinVariant.FixerVariant end,
                      GetWorldPosition = function() return { x = 120, y = -180, z = 8 } end,
                      GetName = function() return "LocKey#4001" end,
                      id = { value = 11 }, worldPosition = { x = 120, y = -180, z = 8 } },
                    { GetVariant = function() return gamedataMappinVariant.CustomPositionVariant end,
                      GetWorldPosition = function() return { x = 400.5, y = 199.75, z = 8 } end,
                      GetName = function() return nil end,
                      id = { value = 12 }, worldPosition = { x = 400.5, y = 199.75, z = 8 } },
                }
            end,
        }
    end,
    GetScriptableSystemsContainer = function()
        return {
            Get = function(self, _name)
                return { districtManager = { GetCurrentDistrict = function()
                    return { GetDistrictID = function() return { value = "watson" } end }
                end } }
            end,
        }
    end,
}

gamedataMappinVariant = { CustomPositionVariant = "CustomPositionVariant",
                          FixerVariant = "FixerVariant" }
gamemappinsMappinTargetType = { Map = 1 }

TweakDBInterface = {
    GetDistrictRecord = function(_id)
        return { LocalizedName = function() return "LocKey#9001" end,
                 SubDistrict = { "LocKey#9002", "LocKey#9003" } }
    end,
}

function GetLocalizedText(key)
    local known = {
        ["LocKey#1234"] = "Deliver the package to Vex",
        ["LocKey#2001"] = "Rogue's request",
        ["LocKey#77"] = "Overture",
        ["LocKey#9001"] = "Watson",
        ["LocKey#4001"] = "Regina Jones",
    }
    return known[key] or key
end

-- A JSON encoder good enough for a flat table of strings and numbers, which is all the mod
-- produces. Independent of CET's own, on purpose: this one only has to be correct.
json = {
    encode = function(value)
        local parts = {}
        for key, item in pairs(value) do
            local encoded
            if type(item) == "number" then
                encoded = tostring(item)
            elseif type(item) == "string" then
                encoded = '"' .. item:gsub('"', '\\"') .. '"'
            else
                encoded = "null"
            end
            parts[#parts + 1] = '"' .. key .. '":' .. encoded
        end
        table.sort(parts)
        return "{" .. table.concat(parts, ",") .. "}"
    end,
}

local handlers = {}
function registerForEvent(name, handler)
    handlers[name] = handler
end

-- --- load the mod and drive it -----------------------------------------------------------

local mod = os.getenv("G13_TEST_MOD")
if not mod then
    print("G13_TEST_MOD is not set")
    os.exit(2)
end
dofile(mod)

truthy("the mod registers onInit", handlers.onInit ~= nil)
truthy("the mod registers onUpdate", handlers.onUpdate ~= nil)

-- A console helper that would blow up on a missing method is worse than useless.
local probed = pcall(G13Probe)
truthy("G13Probe() runs", probed)

handlers.onInit()

local function read_state()
    local file = io.open("hud.json", "r")
    if not file then
        return nil
    end
    local text = file:read("a")
    file:close()
    local state = {}
    for key, value in text:gmatch('"([%w_]+)":("?[^",}]*"?)') do
        state[key] = value:gsub('^"', ""):gsub('"$', "")
    end
    return state
end

local state = read_state()
truthy("onInit wrote hud.json", state ~= nil)
if state then
    check("health came through the stats system", "87", state.health)
    check("level came through", "32", state.level)
    check("street cred came through", "50", state.streetcred)
    check("the objective's loc key was resolved", "Deliver the package to Vex", state.objective)
    check("the quest's loc key was resolved", "Rogue's request", state.quest)
    check("the objective id is there", "q005_rogue_obj_2", state.objective_id)
    check("the weapon's loc key was resolved", "Overture", state.weapon)
    check("the magazine count came through", "24", state.ammo)
    check("the reserve count came through", "180", state.ammo_total)
    check("the player position came through", "100.5", state.x)
    check("heading, facing north (forward y = 1)", "0", state.heading)
    -- The pin is 300 east and 400 north of the player: 500 units, 36 degrees off north.
    check("the pin's bearing came through", "36", state.pin_bearing)
    check("the pin's distance came through", "5", state.pin_distance)
    check("the pin reads ahead when facing north", "36", state.pin_relative)
    check("the district you are in", "Watson", state.district)
    check("the nearest named place", "Regina Jones", state.near)
    check("the ready-made line for the screen", "PIN 5m  Watson", state.route)
end

-- The throttle: a tenth of a second is not enough to redraw, two of them are.
local function stamp()
    local file = io.open("hud.json", "r")
    if not file then return nil end
    local text = file:read("a")
    file:close()
    return text
end

os.remove("hud.json")
handlers.onUpdate(0.1)
check("nothing is written before the interval is up", nil, stamp())
handlers.onUpdate(0.1)
truthy("a write happens once the interval is up", stamp() ~= nil)

-- Facing east: the arrow has to follow.
FORWARD = { x = 1, y = 0, z = 0 }
handlers.onUpdate(0.2)
check("heading, facing east (forward x = 1)", "90", read_state().heading)
check("the pin is now 36 degrees anticlockwise of east, so 306", "306", read_state().pin_relative)

-- A build that refuses the weapon calls must still produce a file: a game must never break
-- the mod, and a missing ammo count is a blank on the screen, not a crash.
REFUSE_WEAPON = true
local survived = pcall(function() handlers.onUpdate(0.2) end)
truthy("a refusing game build does not break the mod", survived)
check("and the fields it cannot read are simply absent", nil, read_state().ammo)

-- Leave a complete file behind: the Python test renders the applet against it afterwards.
REFUSE_WEAPON = false
handlers.onUpdate(0.2)

print(string.format("CET MOD TEST: %s", (FAILURES or 0) == 0 and "all checks passed"
                    or (FAILURES .. " FAILURES")))
os.exit((FAILURES or 0) == 0 and 0 or 1)

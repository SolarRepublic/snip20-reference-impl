import { is_array, is_object, keys } from '@blake.regalia/belt';
export function canonicalize_json(w_json) {
    // recursively canonicalize arrays
    if (is_array(w_json))
        return w_json.map(w => is_object(w) ? canonicalize_json(w) : w);
    // build new json object for canonicalizing
    const h_test = {};
    // each lexicographically sorted key
    for (const si_key of keys(w_json).sort()) {
        // ref its value
        const w_value = w_json[si_key];
        // set value on test object
        h_test[si_key] = is_object(w_value) ? canonicalize_json(w_value) : w_value;
    }
    // return canonicalized object
    return h_test;
}
//# sourceMappingURL=util.js.map
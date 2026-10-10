using UnityEngine;
using UnityEditor;

public static class TutorSetupMenu
{
    [MenuItem("LearningWorlds/Setup Socratic Tutors")]
    public static void Setup()
    {
        // Create either an existing or a new TutorManager
        GameObject tutorManagerGameObject = GameObject.FindObjectOfType<SocraticTutorManager>()?.gameObject;

        if (tutorManagerGameObject == null)
        {
            tutorManagerGameObject = new GameObject("SocraticTutorManager");
            tutorManagerGameObject.AddComponent<SocraticTutorManager>();
        }
        
        SocraticTutorConfig tutorConfig = null;
        
        // Look for an existing configuration asset
        string[] guids = AssetDatabase.FindAssets("t:SocraticTutorConfig");
        
        if (guids.Length > 0)
        {
            string path = AssetDatabase.GUIDToAssetPath(guids[0]);
            tutorConfig = AssetDatabase.LoadAssetAtPath<SocraticTutorConfig>(path);
        }
        else
        {
            // If none exists, create a new one
            tutorConfig = ScriptableObject.CreateInstance<SocraticTutorConfig>();
            string assetPath = "Assets/LearningWorlds/Scripts/Tutors/DefaultSocraticTutorConfig.asset";
            AssetDatabase.CreateAsset(tutorConfig, assetPath);
            
            // Configure a basic set
            tutorConfig.tutorConfigs = new SocraticTutorConfig.TutorConfig[]
            {
                new SocraticTutorConfig.TutorConfig { world = SocraticTutorManager.World.Hub, characterId = "HubTutor", spawnPosition = new Vector3(0, 0, 5), spawnRotation = new Vector3(0, 0, 0) },
                new SocraticTutorConfig.TutorConfig { world = SocraticTutorManager.World.PseGallery, characterId = "PseGalleryTutor", spawnPosition = new Vector3(15, 0, 0), spawnRotation = new Vector3(0, -90, 0) },
                new SocraticTutorConfig.TutorConfig { world = SocraticTutorManager.World.ElementRoom, characterId = "ElementRoomTutor", spawnPosition = new Vector3(0, 0, -15), spawnRotation = new Vector3(0, 180, 0) },
                new SocraticTutorConfig.TutorConfig { world = SocraticTutorManager.World.Arachnophobia, characterId = "ArachnophobiaTutor", spawnPosition = new Vector3(-15, 0, 0), spawnRotation = new Vector3(0, 180, 0) }
            };
        }
        
        // Assign configurations
        SocraticTutorManager tutorManager = tutorManagerGameObject.GetComponent<SocraticTutorManager>();
        tutorManager.tutorConfig = tutorConfig;

        Debug.Log($"[Setup Menu] Socratic Tutors configured as Success with prefs.[tutorManager.tutorConfig]");
    }
}
using UnityEditor;
using UnityEngine;
using UnityEditor.SceneManagement;
using System.IO;

namespace LearningWorlds.Editor
{
    public static class LearningWorldsMenus
    {
        [MenuItem("LearningWorlds/Open Hub Scene", false)]
        public static void OpenHub()
        {
            if (Application.isPlaying) return;

            string scenePath = "Assets/Scenes/LearningWorlds-Hub.unity";
            if (!File.Exists(scenePath))
            {
                Debug.LogError("Scene not found: " + scenePath);
                return;
            }

            EditorSceneManager.NewScene(NewSceneSetup.DefaultGameObjects);
            EditorSceneManager.LoadScene(scenePath);
        }

        [MenuItem("LearningWorlds/Register Build Scenes", false)]
        public static void Register()
        {
            string hubScenePath = "Assets/Scenes/LearningWorlds-Hub.unity";
            if (!File.Exists(hubScenePath))
            {
                Debug.LogError("Scene not found: " + hubScenePath);
                return;
            }

            bool isRegistered = false;
            EditorBuildSettingsScene[] scenes = EditorBuildSettings.scenes;

            for (int i = 0; i < scenes.Length; i++)
            {
                if (scenes[i].path == hubScenePath)
                {
                    isRegistered = true;
                    break;
                }
            }

            if (!isRegistered)
            {
                System.Array.Resize(ref scenes, scenes.Length + 1);
                scenes[scenes.Length - 1] = new EditorBuildSettingsScene(hubScenePath, true);
                EditorBuildSettings.scenes = scenes;
                EditorUtility.DisplayDialog("Success", string.Format("Registered scene: {0}", hubScenePath), "OK");
            }
        }
    }
}
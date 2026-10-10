using System;
using System.Collections.Generic;
using UnityEngine;

public static class BuildingBuilders
{
    private static Dictionary<World, Func<GameObject>> builders = new Dictionary<World, Func<GameObject>>();

    public static void RegisterBuilder(World world, Func<GameObject> builder)
    {
        builders[world] = builder;
    }

    public static GameObject Build(World world, System.Action<Element> buildFunc, Element currentElement = null)
    {
        if (!builders.ContainsKey(world))
        {
            throw new System.ArgumentException("The specified builder was not found.");
        }
         
        return BuildWithElement(builders[world], currentElement);
    }

    private static GameObject BuildWithElement(Func<GameObject> func, Element element)
    {
        return func();  // Adjust this based on actual usage
    }
}